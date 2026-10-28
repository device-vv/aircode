// Reproductions of editor-level bugs. Each test asserts the CORRECT behaviour.
use aircode_editor::Document;

/// Backspace at offset 0 (or Delete at the end) goes through
/// replace_range(empty, "") — this records a no-op edit and wipes redo history.
#[test]
fn bug_noop_edit_clears_redo_history() {
    let mut doc = Document::from_text("abc");
    doc.set_selection(3..3).unwrap();
    doc.replace_selection("d").unwrap();
    assert!(doc.undo());
    // what DocumentView::backspace does at the start of the document:
    doc.replace_range(0..0, "").unwrap();
    assert!(doc.redo(), "redo history was destroyed by a no-op edit");
}

/// The UI splits lines on '\n' only, so in a CRLF file the line's end offset
/// lies BETWEEN '\r' and '\n'. End / click-after-line-end places the caret
/// there and typed text lands after '\r', corrupting the line ending.
/// (Mirrors DocumentView::line_ranges + End.)
#[test]
fn bug_crlf_end_of_line_splits_line_break() {
    let mut doc = Document::from_text("let a = 1;\r\nlet b = 2;\r\n");
    let line_end = doc.text().find('\n').unwrap(); // == DocumentView line_ranges()[0].end
    doc.set_selection(line_end..line_end).unwrap();
    doc.replace_selection(" // x").unwrap();
    assert_eq!(
        doc.text(),
        "let a = 1; // x\r\nlet b = 2;\r\n",
        "text was inserted between CR and LF"
    );
}

/// Up/Down must keep the VISUAL column (characters), not the byte column:
/// from the 3rd Cyrillic letter the caret must land on the 3rd ASCII character below.
#[test]
fn bug_vertical_movement_uses_byte_columns() {
    let doc = Document::from_text("Привет\nabcdefgh");
    let (line, column) = doc.line_col_at("При".len());
    let target = doc.offset_for_line_col(line + 1, column);
    assert_eq!(
        doc.text()[doc.line_range(1).start..target].chars().count(),
        3
    );
    assert_eq!(column, 3);
}

/// CRLF terminators are not part of the line.
#[test]
fn crlf_is_excluded_from_line_ranges() {
    let doc = Document::from_text("ab\r\ncd\r\n");
    assert_eq!(doc.line_range(0), 0..2);
    assert_eq!(doc.line_range(1), 4..6);
    assert_eq!(doc.line_range(2), 8..8);
    assert_eq!(doc.line_count(), 3);
    assert_eq!(doc.line_col_at(3), (0, 2)); // between \r and \n
}

/// Each keystroke is a separate undo record with no limit / coalescing:
/// typing a word needs one Ctrl+Z per character.
#[test]
fn bug_undo_is_per_character() {
    let mut doc = Document::from_text("");
    for ch in ["h", "e", "l", "l", "o"] {
        doc.replace_selection(ch).unwrap();
    }
    assert!(doc.undo());
    assert_eq!(doc.text(), "", "one undo should revert the typed word");
}
