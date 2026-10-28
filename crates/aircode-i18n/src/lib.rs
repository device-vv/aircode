#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Language {
    English,
    Russian,
}

impl Language {
    pub fn from_code(code: &str) -> Self {
        if code.eq_ignore_ascii_case("ru") {
            Self::Russian
        } else {
            Self::English
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Russian => "ru",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Russian => "Русский",
        }
    }
}

pub fn system_is_russian() -> bool {
    sys_locale::get_locale().is_some_and(|locale| locale.to_lowercase().starts_with("ru"))
}

pub fn tr(language: Language, key: &'static str) -> &'static str {
    match (language, key) {
        (Language::English, "open") => "Open",
        (Language::English, "save") => "Save",
        (Language::English, "folder") => "Folder",
        (Language::English, "theme") => "Theme",
        (Language::English, "settings") => "Settings",
        (Language::English, "focus") => "Focus",
        (Language::English, "project") => "Project",
        (Language::English, "files") => "Files and folders",
        (Language::English, "open_folder") => "Open folder",
        (Language::English, "commands") => "Commands and file actions",
        (Language::English, "modified") => "modified",
        (Language::English, "unsaved") => "unsaved",
        (Language::English, "saved") => "Saved",
        (Language::English, "current_document") => "Current document",
        (Language::English, "native_chat") => "Native AI chat",
        (Language::English, "chat_hint") => {
            "Choose Kimi Code and k3.1-pro, save your membership API key, then send a message. Your document is included only when context is enabled."
        }
        (Language::English, "chat_security") => {
            "No CLI, no browser proxy, no keys in settings files."
        }
        (Language::English, "chat_placeholder") => "Ask about your code...",
        (Language::English, "context_on") => "Document context: on",
        (Language::English, "context_off") => "Document context: off",
        (Language::English, "send") => "Send",
        (Language::English, "stop") => "Stop",
        (Language::English, "new_chat") => "New chat",
        (Language::English, "new_session") => "New session",
        (Language::English, "ready") => "Ready",
        (Language::English, "thinking") => "Thinking...",
        (Language::English, "reasoning") => "Reasoning",
        (Language::English, "show_reasoning") => "Show reasoning",
        (Language::English, "hide_reasoning") => "Hide reasoning",
        (Language::English, "copy") => "Copy",
        (Language::English, "cut") => "Cut",
        (Language::English, "paste") => "Paste",
        (Language::English, "select_all") => "Select All",
        (Language::English, "you") => "You",
        (Language::English, "settings_title") => "AI provider settings",
        (Language::English, "close") => "Close",
        (Language::English, "provider") => "Provider",
        (Language::English, "region") => "Region",
        (Language::English, "model") => "Model",
        (Language::English, "model_id") => "Model ID",
        (Language::English, "base_url") => "Base URL",
        (Language::English, "effort") => "Reasoning effort (new session)",
        (Language::English, "kimi_thinking") => {
            "K3.1 Pro always uses thinking. Changing the model or effort starts a new session."
        }
        (Language::English, "api_key_label") => "API key (stored only in the OS credential store)",
        (Language::English, "api_key_placeholder") => "Paste API key",
        (Language::English, "save_key") => "Save key",
        (Language::English, "delete_key") => "Delete key",
        (Language::English, "key_saved") => "Key saved",
        (Language::English, "no_key_saved") => "No key saved",
        (Language::English, "include_document") => "Include document or selection in chat",
        (Language::English, "on") => "On",
        (Language::English, "off") => "Off",
        (Language::English, "language") => "Language",
        (Language::English, "language_offer") => {
            "Интерфейс системы русский. Переключить AIRcode на русский язык?"
        }
        (Language::English, "keep_english") => "Keep English",
        (Language::English, "use_russian") => "Русский",
        (Language::English, "unsaved_title") => "Unsaved changes",
        (Language::English, "unsaved_open") => {
            "Open another file and discard unsaved changes in the current document?"
        }
        (Language::English, "unsaved_close") => {
            "Close AIRcode and discard unsaved changes in the current document?"
        }
        (Language::English, "discard") => "Discard changes",
        (Language::English, "keep_editing") => "Keep editing",
        (Language::English, "permission") => "Agent permissions",
        (Language::English, "ask_all") => "Ask every action",
        (Language::English, "ask_important") => "Ask important only",
        (Language::English, "full_access") => "Full access",
        (Language::English, "allow") => "Allow once",
        (Language::English, "deny") => "Deny",
        (Language::English, "agent_project_required") => {
            "Open a project folder before using agent tools"
        }
        (Language::English, "audit") => "Agent activity",
        (Language::English, "approval_required") => "Approval required",
        (Language::English, "cancelling") => "Cancelling...",
        (Language::English, "cancelled") => "Cancelled",
        (Language::English, "sandbox_error") => "Cannot initialize project sandbox",
        (Language::English, "agent_step_limit") => "Agent stopped after 8 tool steps",
        (Language::English, "command_palette") => "Select a command",
        (Language::English, "palette_filter") => "Type a command...",
        (Language::English, "no_commands") => "No matching commands",
        (Language::English, "palette_open") => "File: open",
        (Language::English, "palette_save") => "File: save",
        (Language::English, "palette_new_chat") => "AI: new chat",
        (Language::English, "palette_settings") => "AI: provider settings",
        (Language::English, "palette_focus") => "Focus: toggle",
        (Language::English, "palette_ai") => "AI: toggle panel",
        (Language::English, "palette_theme") => "Theme: switch",
        (Language::Russian, "open") => "Открыть",
        (Language::Russian, "save") => "Сохранить",
        (Language::Russian, "folder") => "Папка",
        (Language::Russian, "theme") => "Тема",
        (Language::Russian, "settings") => "Настройки",
        (Language::Russian, "focus") => "Фокус",
        (Language::Russian, "project") => "Проект",
        (Language::Russian, "files") => "Файлы и папки",
        (Language::Russian, "open_folder") => "Открыть папку",
        (Language::Russian, "commands") => "Команды и действия с файлами",
        (Language::Russian, "modified") => "изменён",
        (Language::Russian, "unsaved") => "не сохранён",
        (Language::Russian, "saved") => "Сохранено",
        (Language::Russian, "current_document") => "Текущий документ",
        (Language::Russian, "native_chat") => "Встроенный ИИ-чат",
        (Language::Russian, "chat_hint") => {
            "Выберите Kimi Code и k3.1-pro, сохраните ключ подписки и отправьте сообщение. Документ передаётся только при включённом контексте."
        }
        (Language::Russian, "chat_security") => {
            "Без CLI, браузерного прокси и ключей в файлах настроек."
        }
        (Language::Russian, "chat_placeholder") => "Спросите о коде...",
        (Language::Russian, "context_on") => "Контекст документа: вкл.",
        (Language::Russian, "context_off") => "Контекст документа: выкл.",
        (Language::Russian, "send") => "Отправить",
        (Language::Russian, "stop") => "Остановить",
        (Language::Russian, "new_chat") => "Новый чат",
        (Language::Russian, "new_session") => "Новая сессия",
        (Language::Russian, "ready") => "Готово",
        (Language::Russian, "thinking") => "Думаю...",
        (Language::Russian, "reasoning") => "Рассуждение",
        (Language::Russian, "show_reasoning") => "Показать рассуждение",
        (Language::Russian, "hide_reasoning") => "Скрыть рассуждение",
        (Language::Russian, "copy") => "Копировать",
        (Language::Russian, "cut") => "Вырезать",
        (Language::Russian, "paste") => "Вставить",
        (Language::Russian, "select_all") => "Выделить всё",
        (Language::Russian, "you") => "Вы",
        (Language::Russian, "settings_title") => "Настройки ИИ-провайдера",
        (Language::Russian, "close") => "Закрыть",
        (Language::Russian, "provider") => "Провайдер",
        (Language::Russian, "region") => "Регион",
        (Language::Russian, "model") => "Модель",
        (Language::Russian, "model_id") => "Идентификатор модели",
        (Language::Russian, "base_url") => "Базовый URL",
        (Language::Russian, "effort") => "Глубина reasoning (новая сессия)",
        (Language::Russian, "kimi_thinking") => {
            "K3.1 Pro всегда использует thinking. Смена модели или глубины reasoning начинает новую сессию."
        }
        (Language::Russian, "api_key_label") => "API-ключ (только в системном хранилище ОС)",
        (Language::Russian, "api_key_placeholder") => "Вставьте API-ключ",
        (Language::Russian, "save_key") => "Сохранить ключ",
        (Language::Russian, "delete_key") => "Удалить ключ",
        (Language::Russian, "key_saved") => "Ключ сохранён",
        (Language::Russian, "no_key_saved") => "Ключ не сохранён",
        (Language::Russian, "include_document") => "Передавать документ или выделение в чат",
        (Language::Russian, "on") => "Вкл.",
        (Language::Russian, "off") => "Выкл.",
        (Language::Russian, "language") => "Язык",
        (Language::Russian, "unsaved_title") => "Несохранённые изменения",
        (Language::Russian, "unsaved_open") => {
            "Открыть другой файл и отменить несохранённые изменения в текущем документе?"
        }
        (Language::Russian, "unsaved_close") => {
            "Закрыть AIRcode и отменить несохранённые изменения в текущем документе?"
        }
        (Language::Russian, "discard") => "Отменить изменения",
        (Language::Russian, "keep_editing") => "Продолжить редактирование",
        (Language::Russian, "permission") => "Разрешения агента",
        (Language::Russian, "ask_all") => "Спрашивать каждое действие",
        (Language::Russian, "ask_important") => "Спрашивать только важное",
        (Language::Russian, "full_access") => "Полный доступ",
        (Language::Russian, "allow") => "Разрешить один раз",
        (Language::Russian, "deny") => "Запретить",
        (Language::Russian, "agent_project_required") => {
            "Откройте папку проекта перед использованием инструментов агента"
        }
        (Language::Russian, "audit") => "Действия агента",
        (Language::Russian, "approval_required") => "Требуется разрешение",
        (Language::Russian, "cancelling") => "Останавливаю...",
        (Language::Russian, "cancelled") => "Остановлено",
        (Language::Russian, "sandbox_error") => "Не удалось настроить защищённую область проекта",
        (Language::Russian, "agent_step_limit") => "Агент остановлен после 8 вызовов инструментов",
        (Language::Russian, "command_palette") => "Выберите команду",
        (Language::Russian, "palette_filter") => "Введите команду...",
        (Language::Russian, "no_commands") => "Нет подходящих команд",
        (Language::Russian, "palette_open") => "Файл: открыть",
        (Language::Russian, "palette_save") => "Файл: сохранить",
        (Language::Russian, "palette_new_chat") => "ИИ: новый чат",
        (Language::Russian, "palette_settings") => "ИИ: настройки провайдера",
        (Language::Russian, "palette_focus") => "Фокус: переключить",
        (Language::Russian, "palette_ai") => "ИИ: показать или скрыть панель",
        (Language::Russian, "palette_theme") => "Тема: переключить",
        (_, unknown) => unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn russian_ui_is_not_english() {
        assert_eq!(tr(Language::Russian, "settings"), "Настройки");
        assert_eq!(
            tr(Language::Russian, "ask_all"),
            "Спрашивать каждое действие"
        );
        assert_eq!(tr(Language::English, "settings"), "Settings");
    }
}
