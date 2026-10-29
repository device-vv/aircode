use crate::TerminalSize;

/// A terminal color resolved from the VT parser.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalColor {
    Default,
    Indexed(u8),
    Rgb(u8, u8, u8),
}

/// An owned cell suitable for rendering after the parser lock is released.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CellSnapshot {
    /// Empty for blank cells and for the continuation cell of a wide glyph.
    pub contents: String,
    pub foreground: TerminalColor,
    pub background: TerminalColor,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

impl Default for CellSnapshot {
    fn default() -> Self {
        Self {
            contents: String::new(),
            foreground: TerminalColor::Default,
            background: TerminalColor::Default,
            bold: false,
            italic: false,
            underline: false,
            inverse: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CursorSnapshot {
    /// Zero-based visible-grid row.
    pub row: u16,
    /// Zero-based visible-grid column.
    pub column: u16,
    pub visible: bool,
}

/// Owned visible-grid state. The vector is row-major and exactly rows*columns.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalSnapshot {
    pub size: TerminalSize,
    pub cursor: CursorSnapshot,
    pub cells: Vec<CellSnapshot>,
    pub title: String,
    pub alternate_screen: bool,
    /// Number of rows currently retained before the visible screen.
    pub scrollback: usize,
    /// Rows above the live screen represented by this snapshot.
    pub scrollback_offset: usize,
}

impl TerminalSnapshot {
    pub fn cell(&self, row: u16, column: u16) -> Option<&CellSnapshot> {
        if row >= self.size.rows() || column >= self.size.columns() {
            return None;
        }
        let index = usize::from(row) * usize::from(self.size.columns()) + usize::from(column);
        self.cells.get(index)
    }

    /// Convenience for tests and simple views; full renderers should use cells.
    pub fn row_text(&self, row: u16) -> Option<String> {
        if row >= self.size.rows() {
            return None;
        }
        let mut text = String::new();
        for column in 0..self.size.columns() {
            let cell = self.cell(row, column)?;
            if cell.contents.is_empty() {
                text.push(' ');
            } else {
                text.push_str(&cell.contents);
            }
        }
        Some(text)
    }
}

pub(crate) struct Grid {
    parser: vt100::Parser,
}

impl Grid {
    pub(crate) fn new(size: TerminalSize, scrollback_lines: usize) -> Self {
        Self {
            parser: vt100::Parser::new(size.rows(), size.columns(), scrollback_lines),
        }
    }

    pub(crate) fn process(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    pub(crate) fn resize(&mut self, size: TerminalSize) {
        self.parser
            .screen_mut()
            .set_size(size.rows(), size.columns());
    }

    pub(crate) fn application_cursor_keys(&self) -> bool {
        self.parser.screen().application_cursor()
    }

    pub(crate) fn snapshot(&mut self) -> TerminalSnapshot {
        self.snapshot_at(0)
    }

    pub(crate) fn snapshot_at(&mut self, scrollback_offset: usize) -> TerminalSnapshot {
        let previous_offset = self.parser.screen().scrollback();

        // vt100 clamps this value to the available history. Reading it back
        // gives the UI an exact upper bound without exposing the parser.
        self.parser
            .screen_mut()
            .set_scrollback(crate::MAX_SCROLLBACK_LINES);
        let available_scrollback = self.parser.screen().scrollback();
        let scrollback_offset = scrollback_offset.min(available_scrollback);
        self.parser.screen_mut().set_scrollback(scrollback_offset);
        let screen = self.parser.screen();

        let (rows, columns) = screen.size();
        let size = TerminalSize::new(rows, columns)
            .expect("vt100 grid size must remain within terminal API limits");
        let (cursor_row, cursor_column) = screen.cursor_position();
        let mut cells = Vec::with_capacity(usize::from(size.rows()) * usize::from(size.columns()));

        for row in 0..size.rows() {
            for column in 0..size.columns() {
                cells.push(
                    screen
                        .cell(row, column)
                        .map(snapshot_cell)
                        .unwrap_or_default(),
                );
            }
        }

        let snapshot = TerminalSnapshot {
            size,
            cursor: CursorSnapshot {
                row: cursor_row,
                column: cursor_column,
                visible: !screen.hide_cursor() && scrollback_offset == 0,
            },
            cells,
            title: String::new(),
            alternate_screen: screen.alternate_screen(),
            scrollback: available_scrollback,
            scrollback_offset,
        };
        self.parser.screen_mut().set_scrollback(previous_offset);
        snapshot
    }
}

fn snapshot_cell(cell: &vt100::Cell) -> CellSnapshot {
    CellSnapshot {
        contents: cell.contents().to_owned(),
        foreground: snapshot_color(cell.fgcolor()),
        background: snapshot_color(cell.bgcolor()),
        bold: cell.bold(),
        italic: cell.italic(),
        underline: cell.underline(),
        inverse: cell.inverse(),
    }
}

fn snapshot_color(color: vt100::Color) -> TerminalColor {
    match color {
        vt100::Color::Default => TerminalColor::Default,
        vt100::Color::Idx(index) => TerminalColor::Indexed(index),
        vt100::Color::Rgb(red, green, blue) => TerminalColor::Rgb(red, green, blue),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn size() -> TerminalSize {
        TerminalSize::new(6, 30).unwrap()
    }

    fn all_visible_text(snapshot: &TerminalSnapshot) -> String {
        (0..snapshot.size.rows())
            .map(|row| snapshot.row_text(row).unwrap())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn split_utf8_and_vt_sequences_retain_text_and_color() {
        let mut grid = Grid::new(size(), 8);
        let bytes = "\x1b[31mAЖ🦀\x1b[0m".as_bytes();
        for byte in bytes {
            grid.process(&[*byte]);
        }

        let snapshot = grid.snapshot();
        assert_eq!(snapshot.cell(0, 0).unwrap().contents, "A");
        assert_eq!(snapshot.cell(0, 1).unwrap().contents, "Ж");
        assert_eq!(snapshot.cell(0, 2).unwrap().contents, "🦀");
        assert_eq!(
            snapshot.cell(0, 0).unwrap().foreground,
            TerminalColor::Indexed(1)
        );
        assert_eq!(
            snapshot.cell(0, 1).unwrap().foreground,
            TerminalColor::Indexed(1)
        );
        assert_eq!(snapshot.cursor.column, 4);
    }

    #[test]
    fn snapshot_contains_truecolor_indexed_background_and_cursor_state() {
        let mut grid = Grid::new(size(), 0);
        grid.process(b"\x1b[2;3H\x1b[38;2;1;2;3m\x1b[48;5;42mX\x1b[?25l");
        let snapshot = grid.snapshot();
        let cell = snapshot.cell(1, 2).unwrap();
        assert_eq!(cell.contents, "X");
        assert_eq!(cell.foreground, TerminalColor::Rgb(1, 2, 3));
        assert_eq!(cell.background, TerminalColor::Indexed(42));
        assert_eq!(snapshot.cursor.row, 1);
        assert_eq!(snapshot.cursor.column, 3);
        assert!(!snapshot.cursor.visible);

        grid.process(b"\x1b[?25h");
        assert!(grid.snapshot().cursor.visible);
    }

    #[test]
    fn resize_updates_grid_dimensions_and_cell_count() {
        let mut grid = Grid::new(size(), 0);
        grid.process(b"before");
        let new_size = TerminalSize::new(3, 11).unwrap();
        grid.resize(new_size);
        grid.process(b"\x1b[3;11H!");
        let snapshot = grid.snapshot();
        assert_eq!(snapshot.size, new_size);
        assert_eq!(snapshot.cells.len(), 33);
        assert_eq!(snapshot.cell(2, 10).unwrap().contents, "!");
    }

    #[test]
    fn finite_scrollback_and_large_output_keep_latest_visible_lines() {
        let small = TerminalSize::new(4, 20).unwrap();
        let mut grid = Grid::new(small, 3);
        for line in 0..100 {
            grid.process(format!("line{line}\r\n").as_bytes());
        }
        let snapshot = grid.snapshot();
        let text = all_visible_text(&snapshot);
        assert!(text.contains("line99"));
        assert!(!text.contains("line0\r"));
        assert_eq!(snapshot.cells.len(), 80);
    }

    #[test]
    fn snapshots_can_read_history_without_moving_the_live_viewport() {
        let small = TerminalSize::new(4, 20).unwrap();
        let mut grid = Grid::new(small, 20);
        for line in 0..12 {
            grid.process(format!("line{line}\r\n").as_bytes());
        }

        let live = grid.snapshot();
        assert!(live.scrollback > 0);
        assert_eq!(live.scrollback_offset, 0);

        let history = grid.snapshot_at(live.scrollback);
        assert_eq!(history.scrollback_offset, live.scrollback);
        assert!(!history.cursor.visible);
        assert_ne!(history.row_text(0), live.row_text(0));
        assert_eq!(history.scrollback, live.scrollback);

        let clamped = grid.snapshot_at(usize::MAX);
        assert_eq!(clamped.scrollback_offset, live.scrollback);
        assert_eq!(grid.snapshot().scrollback_offset, 0);
    }

    #[test]
    fn decckm_mode_drives_application_arrow_encoding() {
        let mut grid = Grid::new(size(), 0);
        assert!(!grid.application_cursor_keys());
        grid.process(b"\x1b[?1h");
        assert!(grid.application_cursor_keys());
        grid.process(b"\x1b[?1l");
        assert!(!grid.application_cursor_keys());
    }
}
