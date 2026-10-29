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
        (Language::English, "menu") => "Menu",
        (Language::English, "save") => "Save",
        (Language::English, "folder") => "Folder",
        (Language::English, "theme") => "Theme",
        (Language::English, "settings") => "Settings",
        (Language::English, "focus") => "Focus",
        (Language::English, "project") => "Project",
        (Language::English, "project_rust") => "Rust project",
        (Language::English, "project_python") => "Python project",
        (Language::English, "project_dart") => "Dart project",
        (Language::English, "project_flutter") => "Flutter project",
        (Language::English, "project_unknown") => "No project marker",
        (Language::English, "plain_text") => "Plain text",
        (Language::English, "syntax_ready") => "Highlighting ready",
        (Language::English, "syntax_unavailable") => "Highlighting unavailable",
        (Language::English, "syntax_none") => "No syntax grammar",
        (Language::English, "no_language_server") => "No language server",
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
        (Language::English, "clear_history_title") => "Delete project chat history?",
        (Language::English, "clear_history_confirmation") => {
            "This permanently deletes the complete chat and draft for this project. Visible messages cannot be recovered."
        }
        (Language::English, "keep_chat") => "Keep chat",
        (Language::English, "delete_history") => "Delete history",
        (Language::English, "history_load_failed") => "Could not restore project chat",
        (Language::English, "history_save_failed") => "Could not save project chat",
        (Language::English, "history_delete_failed") => "Could not delete project chat",
        (Language::English, "summarizing_context") => "Summarizing earlier conversation...",
        (Language::English, "context_summarized") => "Earlier conversation summarized",
        (Language::English, "context_summary_failed") => "Could not summarize conversation",
        (Language::English, "context_summary_too_large") => {
            "Conversation is too large to summarize without omitting messages"
        }
        (Language::English, "ready") => "Ready",
        (Language::English, "thinking") => "Thinking...",
        (Language::English, "no_response") => "No response was returned.",
        (Language::English, "response_failed") => "Response failed",
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
        (Language::English, "new_file") => "New file…",
        (Language::English, "completion_failed") => {
            "AI completion failed: check provider, model and API key"
        }
        (Language::English, "inline_completion") => {
            "AI code completion (sends code; may incur costs)"
        }
        (Language::English, "on") => "On",
        (Language::English, "off") => "Off",
        (Language::English, "language") => "Language",
        (Language::English, "focus_mode") => "Focus mode",
        (Language::English, "language_offer") => {
            "Интерфейс системы русский. Переключить AIRcode на русский язык?"
        }
        (Language::English, "keep_english") => "Keep English",
        (Language::English, "use_russian") => "Русский",
        (Language::English, "unsaved_title") => "Unsaved changes",
        (Language::English, "file_open_in_another_tab") => {
            "That file is already open in another tab."
        }
        (Language::English, "unsaved_open") => "Close this tab and discard its unsaved changes?",
        (Language::English, "unsaved_close") => {
            "Close AIRcode and discard unsaved changes in all open tabs?"
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
        (Language::English, "agent_step_limit") => {
            "Agent paused after 8 actions. Continue to resume."
        }
        (Language::English, "agent_continue") => "Continue agent",
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
        (Language::English, "palette_terminal") => "Terminal: show or hide",
        (Language::English, "terminal") => "Terminal",
        (Language::English, "hide") => "Hide",
        (Language::English, "terminal_restart") => "Restart",
        (Language::English, "terminal_project_required") => {
            "Open a project before starting the terminal"
        }
        (Language::English, "terminal_idle") => "Terminal is not running",
        (Language::English, "terminal_starting") => "Starting system shell...",
        (Language::English, "terminal_exited") => "Shell exited",
        (Language::English, "terminal_start_failed") => "Could not start terminal",
        (Language::Russian, "open") => "Открыть",
        (Language::Russian, "menu") => "Меню",
        (Language::Russian, "save") => "Сохранить",
        (Language::Russian, "folder") => "Папка",
        (Language::Russian, "theme") => "Тема",
        (Language::Russian, "settings") => "Настройки",
        (Language::Russian, "focus") => "Фокус",
        (Language::Russian, "project") => "Проект",
        (Language::Russian, "project_rust") => "Rust-проект",
        (Language::Russian, "project_python") => "Python-проект",
        (Language::Russian, "project_dart") => "Dart-проект",
        (Language::Russian, "project_flutter") => "Flutter-проект",
        (Language::Russian, "project_unknown") => "Маркер проекта не найден",
        (Language::Russian, "plain_text") => "Обычный текст",
        (Language::Russian, "syntax_ready") => "Подсветка готова",
        (Language::Russian, "syntax_unavailable") => "Подсветка недоступна",
        (Language::Russian, "syntax_none") => "Грамматика не определена",
        (Language::Russian, "no_language_server") => "Языковой сервер не подключён",
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
        (Language::Russian, "clear_history_title") => "Удалить историю чата проекта?",
        (Language::Russian, "clear_history_confirmation") => {
            "Чат и черновик этого проекта будут удалены безвозвратно. Восстановить сообщения будет невозможно."
        }
        (Language::Russian, "keep_chat") => "Оставить чат",
        (Language::Russian, "delete_history") => "Удалить историю",
        (Language::Russian, "history_load_failed") => "Не удалось восстановить чат проекта",
        (Language::Russian, "history_save_failed") => "Не удалось сохранить чат проекта",
        (Language::Russian, "history_delete_failed") => "Не удалось удалить чат проекта",
        (Language::Russian, "summarizing_context") => "Суммирую предыдущую переписку...",
        (Language::Russian, "context_summarized") => "Предыдущая переписка суммирована",
        (Language::Russian, "context_summary_failed") => "Не удалось суммировать переписку",
        (Language::Russian, "context_summary_too_large") => {
            "Переписка слишком велика для суммирования без пропуска сообщений"
        }
        (Language::Russian, "ready") => "Готово",
        (Language::Russian, "thinking") => "Думаю...",
        (Language::Russian, "no_response") => "Ответ не получен.",
        (Language::Russian, "response_failed") => "Не удалось получить ответ",
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
        (Language::Russian, "new_file") => "Создать файл…",
        (Language::Russian, "completion_failed") => {
            "Ошибка подсказок ИИ: проверьте провайдера, модель и ключ"
        }
        (Language::Russian, "inline_completion") => {
            "Подсказки кода ИИ (код отправляется провайдеру; возможна оплата)"
        }
        (Language::Russian, "on") => "Вкл.",
        (Language::Russian, "off") => "Выкл.",
        (Language::Russian, "language") => "Язык",
        (Language::Russian, "focus_mode") => "Режим фокусировки",
        (Language::Russian, "language_offer") => {
            "Интерфейс системы русский. Переключить AIRcode на русский язык?"
        }
        (Language::Russian, "keep_english") => "Оставить английский",
        (Language::Russian, "use_russian") => "Русский",
        (Language::Russian, "unsaved_title") => "Несохранённые изменения",
        (Language::Russian, "file_open_in_another_tab") => "Этот файл уже открыт в другой вкладке.",
        (Language::Russian, "unsaved_open") => {
            "Закрыть вкладку и отменить её несохранённые изменения?"
        }
        (Language::Russian, "unsaved_close") => {
            "Закрыть AIRcode и отменить несохранённые изменения во всех открытых вкладках?"
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
        (Language::Russian, "agent_step_limit") => {
            "Агент приостановлен после 8 действий. Нажмите «Продолжить»."
        }
        (Language::Russian, "agent_continue") => "Продолжить работу агента",
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
        (Language::Russian, "palette_terminal") => "Терминал: показать или скрыть",
        (Language::Russian, "terminal") => "Терминал",
        (Language::Russian, "hide") => "Скрыть",
        (Language::Russian, "terminal_restart") => "Перезапустить",
        (Language::Russian, "terminal_project_required") => {
            "Откройте проект перед запуском терминала"
        }
        (Language::Russian, "terminal_idle") => "Терминал не запущен",
        (Language::Russian, "terminal_starting") => "Запуск системной оболочки...",
        (Language::Russian, "terminal_exited") => "Работа оболочки завершена",
        (Language::Russian, "terminal_start_failed") => "Не удалось запустить терминал",
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

    #[test]
    fn language_offer_is_translated_in_both_languages() {
        for language in [Language::English, Language::Russian] {
            for key in ["language_offer", "keep_english", "use_russian"] {
                assert_ne!(tr(language, key), key);
            }
        }
        assert_eq!(tr(Language::Russian, "keep_english"), "Оставить английский");
    }

    #[test]
    fn syntax_and_project_status_is_translated() {
        let keys = [
            "project_rust",
            "project_python",
            "project_dart",
            "project_flutter",
            "project_unknown",
            "plain_text",
            "syntax_ready",
            "syntax_unavailable",
            "syntax_none",
            "no_language_server",
        ];
        for key in keys {
            assert_ne!(tr(Language::English, key), key);
            assert_ne!(tr(Language::Russian, key), key);
            assert_ne!(tr(Language::English, key), tr(Language::Russian, key));
        }
    }

    #[test]
    fn terminal_controls_and_start_errors_are_translated() {
        let keys = [
            "palette_terminal",
            "terminal",
            "hide",
            "terminal_restart",
            "terminal_project_required",
            "terminal_starting",
            "terminal_exited",
            "terminal_start_failed",
        ];
        for key in keys {
            assert_ne!(tr(Language::English, key), key);
            assert_ne!(tr(Language::Russian, key), key);
            assert_ne!(tr(Language::English, key), tr(Language::Russian, key));
        }
    }
}
