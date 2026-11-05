//! Синхронизация «текстовый буфер редактора <-> yrs-документ» через диффы.
//!
//! Вместо перехвата каждого нажатия клавиши сравниваем текущий текст буфера
//! с последним синхронизированным состоянием: общий префикс/суффикс
//! отрезается, остаётся одна атомарная правка (удаление + вставка).
//! Такой подход переживает вставку из буфера, IME и undo/redo без
//! инструментирования редактора, а CRDT на проводе разруливает гонки.
//!
//! Важно про индексы: буфер `aircode-editor` оперирует **байтовыми**
//! диапазонами, а `yrs::Text` (с `OffsetKind::Utf16`, как у браузерного
//! Yjs) — **UTF-16 code units**. Конверсия — здесь, в одном месте.

/// Атомарная правка текста в байтовых координатах.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextDiff {
    /// Начало заменяемого диапазона (байты).
    pub start: usize,
    /// Сколько байт удалить начиная с `start`.
    pub delete: usize,
    /// Что вставить на место удалённого.
    pub insert: String,
}

/// Дифф `old -> new` как одна правка (общий префикс/суффикс отрезаются).
///
/// Границы всегда лежат на UTF-8 boundary. Составные правки (несколько
/// разрозненных кусков) схлопываются в одну замену — для CRDT это корректно,
/// просто чуть менее экономно по трафику.
pub fn diff_texts(old: &str, new: &str) -> Option<TextDiff> {
    if old == new {
        return None;
    }
    // Общий префикс (байты, с шагом по char boundary).
    let mut prefix = 0;
    for (a, b) in old.bytes().zip(new.bytes()) {
        if a != b {
            break;
        }
        prefix += 1;
    }
    while !old.is_char_boundary(prefix) || !new.is_char_boundary(prefix) {
        prefix -= 1;
    }
    // Общий суффикс, не заходящий на префикс.
    let old_tail = &old[prefix..];
    let new_tail = &new[prefix..];
    let mut suffix = 0;
    for (a, b) in old_tail.bytes().rev().zip(new_tail.bytes().rev()) {
        if a != b {
            break;
        }
        suffix += 1;
    }
    suffix = suffix.min(old_tail.len()).min(new_tail.len());
    while suffix > 0
        && (!old.is_char_boundary(old.len() - suffix) || !new.is_char_boundary(new.len() - suffix))
    {
        suffix -= 1;
    }
    Some(TextDiff {
        start: prefix,
        delete: old.len() - prefix - suffix,
        insert: new[prefix..new.len() - suffix].to_string(),
    })
}

/// Сколько UTF-16 code units в `text[..bytes]` (bytes — char boundary).
///
/// yrs-документ создаётся с `OffsetKind::Utf16` (совместимость с Yjs:
/// браузер считает индексы в UTF-16). Для BMP-символов (кириллица) 1
/// символ = 1 unit, для астральных (emoji) — 2.
pub fn bytes_to_utf16_units(text: &str, bytes: usize) -> u32 {
    text[..bytes]
        .chars()
        .map(|c| c.len_utf16() as u32)
        .sum()
}

/// Связка одного буфера редактора с одним yrs-документом.
///
/// Держит последнюю версию текста, которая точно совпадает с yrs-документом
/// (`synced`). Все локальные/удалённые правки выражаются диффами от неё.
pub struct CollabBinding {
    synced: String,
}

impl CollabBinding {
    /// Новая связка. `initial` — текст, который сейчас и в буфере, и в yrs.
    pub fn new(initial: impl Into<String>) -> Self {
        Self {
            synced: initial.into(),
        }
    }

    /// Текст, синхронизированный с yrs-документом на данный момент.
    pub fn synced(&self) -> &str {
        &self.synced
    }

    /// Принудительно установить синхронизированное состояние
    /// (когда текст сессии изменился удалённой стороной).
    pub fn reset(&mut self, text: String) {
        self.synced = text;
    }

    /// Буфер изменился локально: дифф для применения к yrs-тексту.
    ///
    /// Возвращает `(позиция_в_utf16_units, удалить_units, вставить)` —
    /// готово для `TextRef::remove_range` + `TextRef::insert` при
    /// `OffsetKind::Utf16`.
    pub fn local_change(&mut self, buffer_text: &str) -> Option<(u32, u32, String)> {
        let diff = diff_texts(&self.synced.clone(), buffer_text)?;
        let pos = bytes_to_utf16_units(&self.synced, diff.start);
        let del = bytes_to_utf16_units(
            &self.synced[diff.start..diff.start + diff.delete],
            diff.delete,
        );
        self.synced = buffer_text.to_string();
        Some((pos, del, diff.insert))
    }

    /// В yrs приехали удалённые правки: дифф для применения к буферу.
    ///
    /// `session_text` — актуальный текст yrs-документа, `buffer_text` —
    /// текущий текст буфера. Возвращает `(start, delete, insert)` в байтах
    /// относительно буфера — готово для `Document::apply_remote_edit`.
    /// `None` — буфер уже совпадает с сессией.
    pub fn remote_change(
        &mut self,
        session_text: &str,
        buffer_text: &str,
    ) -> Option<(usize, usize, String)> {
        if buffer_text == session_text {
            self.synced = session_text.to_string();
            return None;
        }
        let diff = diff_texts(buffer_text, session_text)?;
        self.synced = session_text.to_string();
        Some((diff.start, diff.delete, diff.insert))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_change_no_diff() {
        assert_eq!(diff_texts("hello", "hello"), None);
    }

    #[test]
    fn insert_at_end() {
        let d = diff_texts("hello", "hello world").unwrap();
        assert_eq!((d.start, d.delete, d.insert.as_str()), (5, 0, " world"));
    }

    #[test]
    fn delete_in_middle() {
        // общий префикс "hello" (5), суффикс "rld" (3) -> вырезаем " wo"
        let d = diff_texts("hello world", "hellorld").unwrap();
        assert_eq!((d.start, d.delete, d.insert.as_str()), (5, 3, ""));
    }

    #[test]
    fn replace_middle() {
        let d = diff_texts("hello world", "hello Rust").unwrap();
        assert_eq!((d.start, d.delete, d.insert.as_str()), (6, 5, "Rust"));
    }

    #[test]
    fn full_replace() {
        let d = diff_texts("abc", "xyz").unwrap();
        assert_eq!((d.start, d.delete, d.insert.as_str()), (0, 3, "xyz"));
    }

    #[test]
    fn unicode_boundaries() {
        // 🦀 = 4 байта; дифф не должен резать его пополам.
        let d = diff_texts("a🦀b", "a🦀🦀b").unwrap();
        assert_eq!((d.start, d.delete, d.insert.as_str()), (5, 0, "🦀"));
        let d = diff_texts("привет", "привет, мир").unwrap();
        assert_eq!((d.start, d.delete, d.insert.as_str()), (12, 0, ", мир"));
    }

    #[test]
    fn local_change_counts_utf16_units_not_bytes() {
        // "🦀" — 4 байта и 2 UTF-16 units (как в Yjs).
        let mut binding = CollabBinding::new("🦀x".to_string());
        let (pos, del, insert) = binding.local_change("🦀xy").unwrap();
        assert_eq!((pos, del, insert.as_str()), (3, 0, "y"));

        let mut binding = CollabBinding::new("🦀🦀".to_string());
        let (pos, del, insert) = binding.local_change("🦀").unwrap();
        assert_eq!((pos, del, insert.as_str()), (2, 2, ""));
    }

    #[test]
    fn local_change_cyrillic_mid_text_positions() {
        // Регрессия: с OffsetKind::Bytes индексы yrs — байты, и правка
        // «в конец текста» с кириллицей уезжала в середину строки.
        let base = "/ из crate aircode-collab\n/ строка от Rust-клиента\nprint('строка от JS-клиента')\n";
        let mut binding = CollabBinding::new(base.to_string());
        let marker = "\n// actor-smoke was here\n";
        let (pos, del, insert) = binding
            .local_change(&format!("{base}{marker}"))
            .unwrap();
        // Кириллица в BMP: 1 символ = 1 unit. Позиция = длина в символах.
        assert_eq!(pos as usize, base.chars().count());
        assert_eq!((del, insert.as_str()), (0, marker));
        // И посередине текста: вставка после "print('".
        let mut binding = CollabBinding::new(base.to_string());
        let edited = base.replacen("print('", "print('X", 1);
        let (pos, del, insert) = binding.local_change(&edited).unwrap();
        assert_eq!((pos, del, insert.as_str()), (58, 0, "X"));
    }

    #[test]
    fn remote_change_gives_byte_range_for_buffer() {
        let mut binding = CollabBinding::new("abc".to_string());
        let (start, delete, insert) = binding.remote_change("abcXYZ", "abc").unwrap();
        assert_eq!((start, delete, insert.as_str()), (3, 0, "XYZ"));
        assert_eq!(binding.synced(), "abcXYZ");
        // Повторный вызов с уже применённым буфером — None.
        assert_eq!(binding.remote_change("abcXYZ", "abcXYZ"), None);
    }

    #[test]
    fn ping_pong_local_then_remote() {
        let mut binding = CollabBinding::new("ab".to_string());
        // Локальная правка…
        assert!(binding.local_change("aXb").is_some());
        // …а теперь пришла удалённая правка поверх (другой участник дописал).
        let (start, delete, insert) = binding.remote_change("aXb!", "aXb").unwrap();
        assert_eq!((start, delete, insert.as_str()), (3, 0, "!"));
        assert_eq!(binding.synced(), "aXb!");
    }
}
