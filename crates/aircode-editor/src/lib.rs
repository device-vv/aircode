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
        }
    }

    pub fn from_text(text: impl Into<String>) -> Self {
        let mut document = Self::new();
        document.text = text.into();
        document.saved_text = document.text.clone();
        document.selection = document.text.len()..document.text.len();
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
        let selection_before = self.selection.clone();
        let deleted = self.text[range.clone()].to_string();
        self.text.replace_range(range.clone(), replacement);
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
        self.selection = edit.selection_before.clone();
        self.redo.push(edit);
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(edit) = self.redo.pop() else {
            return false;
        };
        self.text.replace_range(edit.range.clone(), &edit.inserted);
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
        let offset = self.selection.end;
        let before = &self.text[..offset];
        let line = before.bytes().filter(|byte| *byte == b'\n').count() + 1;
        let line_start = before.rfind('\n').map(|index| index + 1).unwrap_or(0);
        let column = self.text[line_start..offset].chars().count() + 1;
        (line, column)
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
}
