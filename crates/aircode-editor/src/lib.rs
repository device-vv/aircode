use std::{
    error::Error,
    fmt, fs,
    ops::Range,
    path::{Path, PathBuf},
};

use unicode_segmentation::UnicodeSegmentation;

const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Debug)]
pub enum DocumentError {
    Io(String),
    TooLarge(u64),
    InvalidRange,
}

impl fmt::Display for DocumentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(message) => write!(formatter, "{message}"),
            Self::TooLarge(size) => write!(
                formatter,
                "File is too large for this version: {size} bytes"
            ),
            Self::InvalidRange => write!(formatter, "Text range is not on UTF-8 boundaries"),
        }
    }
}

impl Error for DocumentError {}

#[derive(Clone)]
struct EditRecord {
    range: Range<usize>,
    deleted: String,
    inserted: String,
    selection_before: Range<usize>,
    selection_after: Range<usize>,
}

pub struct Document {
    text: String,
    saved_text: String,
    path: Option<PathBuf>,
    selection: Range<usize>,
    undo: Vec<EditRecord>,
    redo: Vec<EditRecord>,
    line_starts: Vec<usize>,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

impl Document {
    pub fn new() -> Self {
        Self {
            text: String::new(),
            saved_text: String::new(),
            path: None,
            selection: 0..0,
            undo: Vec::new(),
            redo: Vec::new(),
            line_starts: vec![0],
        }
    }

    pub fn from_text(text: impl Into<String>) -> Self {
        let mut document = Self::new();
        document.text = text.into();
        document.saved_text = document.text.clone();
        document.selection = document.text.len()..document.text.len();
        document.reindex();
        document
    }

    pub fn open(path: impl AsRef<Path>) -> Result<Self, DocumentError> {
        let path = path.as_ref();
        let metadata = fs::metadata(path).map_err(io_error)?;
        if metadata.len() > MAX_FILE_BYTES {
            return Err(DocumentError::TooLarge(metadata.len()));
        }
        let text = fs::read_to_string(path).map_err(io_error)?;
        let mut document = Self::from_text(text);
        document.path = Some(path.to_path_buf());
        Ok(document)
    }

    pub fn save(&mut self) -> Result<(), DocumentError> {
        let path = self
            .path
            .clone()
            .ok_or_else(|| DocumentError::Io("Choose a file path before saving".into()))?;
        self.save_as(path)
    }

    pub fn save_as(&mut self, path: impl AsRef<Path>) -> Result<(), DocumentError> {
        let path = path.as_ref();
        fs::write(path, self.text.as_bytes()).map_err(io_error)?;
        self.path = Some(path.to_path_buf());
        self.saved_text = self.text.clone();
        Ok(())
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn file_name(&self) -> &str {
        self.path
            .as_deref()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .unwrap_or("untitled.txt")
    }

    pub fn is_modified(&self) -> bool {
        self.text != self.saved_text
    }

    fn reindex(&mut self) {
        self.line_starts.clear();
        self.line_starts.push(0);
        self.line_starts.extend(
            self.text
                .bytes()
                .enumerate()
                .filter(|(_, byte)| *byte == b'\n')
                .map(|(index, _)| index + 1),
        );
    }

    /// Number of lines. An empty document still has one editable line.
    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    /// Byte range of a line, excluding its LF or CRLF terminator.
    pub fn line_range(&self, line: usize) -> Range<usize> {
        let line = line.min(self.line_starts.len().saturating_sub(1));
        let start = self.line_starts[line];
        match self.line_starts.get(line + 1) {
            Some(next) => {
                let mut end = next - 1;
                if end > start && self.text.as_bytes()[end - 1] == b'\r' {
                    end -= 1;
                }
                start..end
            }
            None => start..self.text.len(),
        }
    }

    /// Line containing an offset. Offsets inside a CRLF belong to that line.
    pub fn line_of_offset(&self, offset: usize) -> usize {
        self.line_starts
            .partition_point(|start| *start <= offset)
            .saturating_sub(1)
    }

    /// Zero-based line and Unicode-scalar column, clamped to the document.
    pub fn line_col_at(&self, offset: usize) -> (usize, usize) {
        let offset = offset.min(self.text.len());
        let line = self.line_of_offset(offset);
        let range = self.line_range(line);
        let mut end = offset.clamp(range.start, range.end);
        while !self.text.is_char_boundary(end) {
            end -= 1;
        }
        (line, self.text[range.start..end].chars().count())
    }

    /// Inverse of [`Self::line_col_at`], clamped to the requested line.
    pub fn offset_for_line_col(&self, line: usize, column: usize) -> usize {
        let line = line.min(self.line_starts.len().saturating_sub(1));
        let range = self.line_range(line);
        self.text[range.clone()]
            .char_indices()
            .nth(column)
            .map(|(index, _)| range.start + index)
            .unwrap_or(range.end)
    }

    pub fn selection(&self) -> Range<usize> {
        self.selection.clone()
    }

    pub fn selected_text(&self) -> &str {
        &self.text[self.selection.clone()]
    }

    pub fn set_selection(&mut self, range: Range<usize>) -> Result<(), DocumentError> {
        if range.start > range.end
            || range.end > self.text.len()
            || !self.text.is_char_boundary(range.start)
            || !self.text.is_char_boundary(range.end)
        {
            return Err(DocumentError::InvalidRange);
        }
        self.selection = range;
        Ok(())
    }

    pub fn replace_selection(&mut self, replacement: &str) -> Result<(), DocumentError> {
        self.replace_range(self.selection.clone(), replacement)
    }

    pub fn replace_range(
        &mut self,
        range: Range<usize>,
        replacement: &str,
    ) -> Result<(), DocumentError> {
        if range.start > range.end
            || range.end > self.text.len()
            || !self.text.is_char_boundary(range.start)
            || !self.text.is_char_boundary(range.end)
        {
            return Err(DocumentError::InvalidRange);
        }
        if range.is_empty() && replacement.is_empty() {
            return Ok(());
        }
        let mut range = range;
        // Do not let an insertion between CR and LF split the line ending.
        if range.is_empty()
            && range.start > 0
            && self.text.as_bytes()[range.start - 1] == b'\r'
            && self.text.as_bytes().get(range.start) == Some(&b'\n')
        {
            range = range.start - 1..range.start - 1;
        }
        let selection_before = self.selection.clone();
        let deleted = self.text[range.clone()].to_string();
        self.text.replace_range(range.clone(), replacement);
        self.reindex();
        let cursor = range.start + replacement.len();
        self.selection = cursor..cursor;
        self.undo.push(EditRecord {
            range,
            deleted,
            inserted: replacement.into(),
            selection_before,
            selection_after: self.selection.clone(),
        });
        self.redo.clear();
        Ok(())
    }

    pub fn undo(&mut self) -> bool {
        let Some(edit) = self.undo.pop() else {
            return false;
        };
        self.text.replace_range(
            edit.range.start..edit.range.start + edit.inserted.len(),
            &edit.deleted,
        );
        self.reindex();
        self.selection = edit.selection_before.clone();
        self.redo.push(edit);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(edit) = self.redo.pop() else {
            return false;
        };
        self.text.replace_range(edit.range.clone(), &edit.inserted);
        self.reindex();
        self.selection = edit.selection_after.clone();
        self.undo.push(edit);
        true
    }

    pub fn previous_boundary(&self, offset: usize) -> usize {
        self.text[..offset]
            .grapheme_indices(true)
            .next_back()
            .map(|(index, _)| index)
            .unwrap_or(0)
    }

    pub fn next_boundary(&self, offset: usize) -> usize {
        self.text[offset..]
            .grapheme_indices(true)
            .nth(1)
            .map(|(index, _)| offset + index)
            .unwrap_or(self.text.len())
    }

    pub fn line_col(&self) -> (usize, usize) {
        let (line, column) = self.line_col_at(self.selection.end);
        (line + 1, column + 1)
    }
}

fn io_error(error: std::io::Error) -> DocumentError {
    DocumentError::Io(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_replacement_and_undo_restore_text() {
        let mut document = Document::from_text("a🦀b");
        document.set_selection(1..5).unwrap();

        document.replace_selection("Rust").unwrap();

        assert_eq!(document.text(), "aRustb");
        assert!(document.is_modified());
        assert!(document.undo());
        assert_eq!(document.text(), "a🦀b");
        assert!(!document.is_modified());
        assert!(document.redo());
        assert_eq!(document.text(), "aRustb");
    }

    #[test]
    fn grapheme_navigation_never_splits_a_emoji() {
        let document = Document::from_text("a🦀b");

        assert_eq!(document.next_boundary(1), 5);
        assert_eq!(document.previous_boundary(5), 1);
    }

    #[test]
    fn rejects_invalid_utf8_ranges() {
        let mut document = Document::from_text("🦀");

        assert!(document.set_selection(1..4).is_err());
        assert!(document.replace_range(0..2, "x").is_err());
    }

    #[test]
    fn save_and_open_round_trip_crlf() {
        let path =
            std::env::temp_dir().join(format!("aircode-editor-test-{}.txt", std::process::id()));
        let mut document = Document::from_text("first\r\nsecond\r\n");
        document
            .set_selection(document.text().len()..document.text().len())
            .unwrap();
        document.replace_selection("third").unwrap();

        document.save_as(&path).unwrap();
        assert!(!document.is_modified());
        let opened = Document::open(&path).unwrap();

        assert_eq!(opened.text(), "first\r\nsecond\r\nthird");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn line_and_column_are_one_based() {
        let mut document = Document::from_text("a\n🦀");
        document
            .set_selection(document.text().len()..document.text().len())
            .unwrap();

        assert_eq!(document.line_col(), (2, 2));
    }

    #[test]
    fn indexed_lines_handle_crlf_and_unicode_columns() {
        let document = Document::from_text("a🦀\r\nsecond\n");
        assert_eq!(document.line_count(), 3);
        assert_eq!(document.line_range(0), 0..5);
        assert_eq!(document.line_range(1), 7..13);
        assert_eq!(document.line_of_offset(6), 0);
        assert_eq!(document.line_col_at(1), (0, 1));
        assert_eq!(document.line_col_at(2), (0, 1));
        assert_eq!(document.line_col_at(5), (0, 2));
        assert_eq!(document.offset_for_line_col(0, 2), 5);
        assert_eq!(document.offset_for_line_col(1, 6), 13);
    }

    #[test]
    fn line_col_clamps_to_utf8_boundaries() {
        let document = Document::from_text("Я🙂\nб");

        assert_eq!(document.line_col_at(1), (0, 0));
        assert_eq!(document.line_col_at(3), (0, 1));
        assert_eq!(document.line_col_at(5), (0, 1));
        assert_eq!(document.line_col_at(6), (0, 2));
        assert_eq!(document.line_col_at(usize::MAX), (1, 1));
    }

    #[test]
    fn insertion_cannot_split_crlf_and_undo_reindexes() {
        let mut document = Document::from_text("a\r\nb");
        document.set_selection(2..2).unwrap();
        document.replace_selection("x").unwrap();
        assert_eq!(document.text(), "ax\r\nb");
        assert_eq!(document.line_count(), 2);
        assert!(document.undo());
        assert_eq!(document.text(), "a\r\nb");
        assert_eq!(document.line_range(0), 0..1);
        assert!(document.redo());
        assert_eq!(document.text(), "ax\r\nb");
        assert_eq!(document.line_range(0), 0..2);
    }

    #[test]
    fn line_index_works_past_ten_thousand_lines() {
        let text = (0..10_050)
            .map(|line| format!("line-{line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let document = Document::from_text(text);
        let range = document.line_range(9999);
        assert_eq!(document.line_count(), 10_050);
        assert_eq!(&document.text()[range.clone()], "line-9999");
        assert_eq!(document.line_of_offset(range.start), 9999);
        assert_eq!(document.line_of_offset(range.end), 9999);
    }
}
