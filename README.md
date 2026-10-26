# AIRcode

**A lightweight, native, cross-platform IDE written in Rust.**
**Лёгкая нативная кроссплатформенная IDE, написанная на Rust.**

[English](#english) · [Русский](#русский)

![AIRcode Focus Workspace prototype](docs/assets/aircode-prototype.png)

## English

AIRcode is an early-stage desktop IDE for Windows, macOS, and Linux. Its goal is to combine a fast, distraction-free editing experience with flexible AI providers while remaining fully usable offline.

The project uses a native GPU interface built with GPUI — no Electron or embedded Chromium. Zed is a performance and functionality reference, not a visual template: AIRcode has its own **Focus Workspace** design where secondary panels appear only when needed.

### Current status

This repository currently contains a **Stage 0 interactive UI prototype**, not a complete text editor or a production-ready IDE.

Implemented:

- Native GPUI application and modular Rust workspace.
- Project, search, Git, AI, and terminal panel layouts.
- Focus mode that hides secondary panels without losing their state.
- Interactive command palette.
- Light and dark themes with shared design tokens.
- Platform-independent workspace state with unit tests.
- CI configuration for Windows, macOS, and Linux.

Planned next:

- Real file editing, rope buffers, and large-file virtualization.
- Tree-sitter syntax highlighting for popular programming languages.
- LSP diagnostics, completion, navigation, formatting, and code actions.
- Integrated terminal, project search, and Git operations.
- Native DeepSeek and RouterAI profiles.
- DeepSeek thinking, Search, and Fetch support.
- OpenAI-compatible endpoints with a configurable `base_url`.
- Streaming chat, explicit context selection, and reviewable diffs.

AI requests, file editing, language servers, and terminal processes shown in the prototype are currently illustrative unless listed as implemented above.

### Build and run

Requirements:

- Rust 1.90 or newer.
- Platform dependencies required by [GPUI](https://crates.io/crates/gpui).

```bash
git clone https://github.com/device-vv/aircode.git
cd aircode
cargo run -p aircode-app
```

### Keyboard shortcuts

Use `Ctrl` on Windows/Linux and `Cmd` on macOS.

| Action | Shortcut |
| --- | --- |
| Command palette | `Ctrl/Cmd + P` |
| Toggle terminal | `Ctrl/Cmd + J` |
| Toggle AI panel | `Ctrl/Cmd + Shift + I` |
| Toggle Focus mode | `Ctrl/Cmd + Shift + F` |
| Toggle light/dark theme | `Ctrl/Cmd + Shift + T` |
| Close command palette | `Esc` |

### Validation

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

The current prototype has been built and exercised on Windows. macOS and Linux builds are configured in CI and still require platform-specific runtime validation.

### Project structure

```text
crates/aircode-app    Application entry point
crates/aircode-core   Platform-independent workspace state
crates/aircode-ui     GPUI interface, themes, and panels
.github/workflows     Cross-platform CI
```

### Security and privacy

The current prototype does not require API keys. Future AI credentials will be stored in the operating system's credential store and excluded from logs. Build artifacts, local screenshots, and other files under `target/` are not tracked by Git.

### License

Apache-2.0. See [LICENSE](LICENSE).

---

## Русский

AIRcode — IDE на ранней стадии разработки для Windows, macOS и Linux. Цель проекта — объединить быстрое редактирование без лишних отвлечений с гибкими ИИ-провайдерами, сохранив полноценную работу офлайн.

Интерфейс нативный и использует GPU через GPUI — без Electron и встроенного Chromium. Zed является ориентиром по производительности и возможностям, но не визуальным шаблоном: у AIRcode собственный дизайн **Focus Workspace**, в котором второстепенные панели появляются только по необходимости.

### Текущий статус

Сейчас репозиторий содержит **интерактивный UI-прототип этапа 0**, а не законченный текстовый редактор или готовую IDE.

Уже реализовано:

- Нативное GPUI-приложение и модульный Rust workspace.
- Компоновка панелей проекта, поиска, Git, ИИ и терминала.
- Focus mode, скрывающий второстепенные панели без потери их состояния.
- Интерактивная палитра команд.
- Светлая и тёмная темы с общими дизайн-токенами.
- Платформенно-независимое состояние рабочего пространства и юнит-тесты.
- CI для Windows, macOS и Linux.

Следующие этапы:

- Реальное редактирование файлов, rope-буферы и виртуализация больших файлов.
- Подсветка популярных языков через Tree-sitter.
- Диагностика, автодополнение, навигация, форматирование и действия LSP.
- Встроенный терминал, поиск по проекту и операции Git.
- Нативные профили DeepSeek и RouterAI.
- DeepSeek thinking, Search и Fetch.
- OpenAI-совместимые API с настраиваемым `base_url`.
- Потоковый чат, явный выбор контекста и просмотр diff перед применением.

ИИ-запросы, редактирование, языковые серверы и процессы терминала, показанные в прототипе, являются демонстрационными, если они не перечислены среди реализованных возможностей.

### Сборка и запуск

Требования:

- Rust 1.90 или новее.
- Системные зависимости [GPUI](https://crates.io/crates/gpui) для вашей ОС.

```bash
git clone https://github.com/device-vv/aircode.git
cd aircode
cargo run -p aircode-app
```

### Горячие клавиши

Используйте `Ctrl` в Windows/Linux и `Cmd` в macOS.

| Действие | Сочетание |
| --- | --- |
| Палитра команд | `Ctrl/Cmd + P` |
| Показать или скрыть терминал | `Ctrl/Cmd + J` |
| Показать или скрыть панель ИИ | `Ctrl/Cmd + Shift + I` |
| Включить или выключить Focus mode | `Ctrl/Cmd + Shift + F` |
| Переключить светлую или тёмную тему | `Ctrl/Cmd + Shift + T` |
| Закрыть палитру команд | `Esc` |

### Проверка

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Прототип собран и проверен в Windows. Сборки macOS и Linux настроены в CI, но фактическая проверка работы на этих платформах ещё предстоит.

### Структура проекта

```text
crates/aircode-app    Точка входа приложения
crates/aircode-core   Платформенно-независимое состояние
crates/aircode-ui     GPUI-интерфейс, темы и панели
.github/workflows     Кроссплатформенный CI
```

### Безопасность и приватность

Текущему прототипу не нужны API-ключи. В дальнейшем учётные данные ИИ будут храниться в системном хранилище ОС и исключаться из логов. Сборочные артефакты, локальные снимки и другие файлы в `target/` не отслеживаются Git.

### Лицензия

Apache-2.0. См. [LICENSE](LICENSE).
