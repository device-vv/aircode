# AIRcode

**A lightweight native code editor and AI workspace written in Rust.**
**Лёгкий нативный редактор кода и ИИ-рабочее пространство на Rust.**

[English](#english) · [Русский](#русский)

![AIRcode with native Kimi K3.1 Pro](docs/assets/aircode-prototype.png)

## English

AIRcode combines a native GPUI editor with direct, streaming AI integrations. Version **0.2.0** is a working Windows x86_64 release: it opens, edits, and saves text files and provides native Kimi Code support, including **K3.1 Pro**.

No Electron, embedded browser, CLI agent, or protocol-translating proxy is used. API keys stay in the operating system's credential store.

### Download Windows

Download `aircode-windows-x86_64.zip` from the [latest GitHub Release](https://github.com/device-vv/aircode/releases/latest), extract it, and run `aircode.exe`. Rust and Cargo are not required.

The executable is portable and currently **unsigned**, so Windows SmartScreen may ask for confirmation. Verify the SHA256 file published beside the package.

### Implemented in v0.2.0

- Open, edit, and save UTF-8 text files with native file dialogs.
- Unicode-aware cursor and selection, Windows IME input, clipboard, undo/redo, and modified state.
- Warning before discarding unsaved changes.
- Streaming AI chat with separate reasoning and answer output, cancellation, and new sessions.
- Optional document or selection context, limited to 200 KB per request.
- Native provider presets, custom model/base URL fields where supported, and light/dark themes.
- 27 unit and contract tests plus an exercised Windows UI and mock SSE workflow.

Not included yet: LSP, Tree-sitter, Git operations, an integrated terminal, autonomous agents, shell command execution, and signed installers. Files larger than 16 MiB are rejected by this version. macOS and Linux remain compile targets, not published runtime releases.

### Native Kimi integration

AIRcode uses the official Kimi Code Chat Completions API directly:

- China: `https://api.kimi.com/coding/v1`
- Overseas: `https://api.kimi.ai/coding/v1`

Available model IDs:

- `k3.1-pro`
- `k3.1-pro-highspeed`
- `k3`
- `k3-256k`
- `kimi-for-coding`
- `kimi-for-coding-highspeed`

K3.1 Pro always uses thinking. `low`, `high`, and `max` reasoning effort levels are available; `high` is the default. Changing the model or effort starts a new session, as recommended by the official documentation. Model availability and context limits depend on the membership tier.

Setup:

1. Create a membership API key in the Kimi Code Console.
2. Open **Settings** in AIRcode.
3. Select **Kimi Code**, your region, and `k3.1-pro`.
4. Paste the key and choose **Save key**.
5. Close Settings, write a message, and choose **Send** or press `Ctrl/Cmd + Enter`.

A separate **Kimi Platform** profile supports the official Moonshot China and overseas endpoints with a manually entered model ID. Coding Plan model IDs are not silently applied to Platform accounts.

Official references: [Kimi Code overview](https://www.kimi.com/code/docs/en/) and [model configuration](https://www.kimi.com/code/docs/en/kimi-code/models.html).

### Other providers

- **DeepSeek** with the official preset.
- **RouterAI** with `https://routerai.ru/api/v1` and `provider/model` IDs.
- **OpenAI-compatible** services with a custom `base_url` and model ID.

The integration uses native HTTP/SSE with an honest `AIRcode/<version>` User-Agent. Contract tests use local mock servers. Live access depends on your account and key; a real paid provider account was not used for release validation.

### Security

- API keys are saved in Windows Credential Manager or the equivalent OS backend.
- Settings contain only provider, region, URL, model, effort, and context preferences.
- Keys are excluded from logs, errors, Git, and release archives.
- Nothing is sent to a provider until you explicitly send a message.
- Document context can be disabled in Settings.

### Build from source

Requires Rust 1.90 or newer and the platform dependencies of GPUI.

```bash
git clone https://github.com/device-vv/aircode.git
cd aircode
cargo run -p aircode-app
```

Validation:

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release --target x86_64-pc-windows-msvc
```

### Keyboard shortcuts

Use `Ctrl` on Windows/Linux and `Cmd` on macOS.

| Action | Shortcut |
| --- | --- |
| Open file | `Ctrl/Cmd + O` |
| Save file | `Ctrl/Cmd + S` |
| Command palette | `Ctrl/Cmd + P` |
| Send chat message | `Ctrl/Cmd + Enter` |
| New chat | `Ctrl/Cmd + Shift + N` |
| Toggle AI panel | `Ctrl/Cmd + Shift + I` |
| Focus mode | `Ctrl/Cmd + Shift + F` |
| Light/dark theme | `Ctrl/Cmd + Shift + T` |
| Provider settings | `Ctrl/Cmd + ,` |
| Close overlay | `Esc` |

### Project structure

```text
crates/aircode-ai       Providers, credentials, settings, HTTP and SSE
crates/aircode-app      aircode.exe entry point
crates/aircode-core     Workspace state
crates/aircode-editor   Unicode document, file I/O, undo/redo
crates/aircode-ui       GPUI editor, inputs, chat and settings
.github/workflows       CI and Windows releases
```

### License

Apache-2.0. See [LICENSE](LICENSE).

---

## Русский

AIRcode объединяет нативный GPUI-редактор и прямые потоковые ИИ-интеграции. Версия **0.2.0** — рабочий релиз для Windows x86_64: открывает, редактирует и сохраняет текстовые файлы и нативно поддерживает Kimi Code, включая **K3.1 Pro**.

Без Electron, встроенного браузера, CLI-агентов и прокси для преобразования протоколов. API-ключи хранятся в системном хранилище ОС.

### Скачать для Windows

Скачайте `aircode-windows-x86_64.zip` из [последнего GitHub Release](https://github.com/device-vv/aircode/releases/latest), распакуйте и запустите `aircode.exe`. Rust и Cargo не требуются.

Приложение portable и пока **не имеет цифровой подписи**, поэтому Windows SmartScreen может запросить подтверждение. Проверьте SHA256-файл рядом с пакетом.

### Реализовано в v0.2.0

- Открытие, редактирование и сохранение UTF-8 файлов через нативные диалоги.
- Курсор и выделение с учётом Unicode, Windows IME, буфер обмена, undo/redo и признак изменений.
- Предупреждение перед потерей несохранённых изменений.
- Потоковый чат с отдельным reasoning и ответом, отменой и новыми сессиями.
- Контекст документа или выделения по выбору пользователя, до 200 КБ на запрос.
- Нативные профили, настройка model/base URL там, где поддерживается, светлая и тёмная темы.
- 27 unit/contract-тестов и проверенный Windows UI со streaming mock.

Ещё не входят: LSP, Tree-sitter, Git, встроенный терминал, автономные агенты, запуск shell-команд и подписанные установщики. Файлы больше 16 МиБ эта версия отклоняет. macOS и Linux остаются целями компиляции, но не опубликованными релизами.

### Нативная интеграция Kimi

AIRcode напрямую использует официальный Kimi Code Chat Completions API:

- Китай: `https://api.kimi.com/coding/v1`
- Другие регионы: `https://api.kimi.ai/coding/v1`

Доступные идентификаторы моделей:

- `k3.1-pro`
- `k3.1-pro-highspeed`
- `k3`
- `k3-256k`
- `kimi-for-coding`
- `kimi-for-coding-highspeed`

K3.1 Pro всегда использует thinking. Доступны `low`, `high` и `max`, по умолчанию — `high`. Смена модели или effort начинает новую сессию согласно официальной документации. Доступность моделей и лимит контекста зависят от подписки.

Настройка:

1. Создайте membership API key в Kimi Code Console.
2. Откройте **Settings** в AIRcode.
3. Выберите **Kimi Code**, регион и `k3.1-pro`.
4. Вставьте ключ и нажмите **Save key**.
5. Закройте настройки, напишите сообщение и нажмите **Send** или `Ctrl/Cmd + Enter`.

Отдельный профиль **Kimi Platform** поддерживает официальные Moonshot endpoint'ы с ручным вводом model ID. Идентификаторы Coding Plan автоматически не применяются к Platform.

Официальные источники: [обзор Kimi Code](https://www.kimi.com/code/docs/en/) и [настройка моделей](https://www.kimi.com/code/docs/en/kimi-code/models.html).

### Другие провайдеры

- **DeepSeek** с официальным preset.
- **RouterAI** с `https://routerai.ru/api/v1` и идентификаторами `provider/model`.
- **OpenAI-совместимые** сервисы с пользовательскими `base_url` и model ID.

Используется нативный HTTP/SSE и честный `AIRcode/<version>` User-Agent. Контрактные тесты работают с локальными mock-серверами. Доступ зависит от вашего аккаунта и ключа; реальный платный аккаунт для проверки релиза не использовался.

### Безопасность

- API-ключи сохраняются в Windows Credential Manager или эквивалентном backend ОС.
- В настройках находятся только провайдер, регион, URL, модель, effort и контекст.
- Ключи не попадают в логи, ошибки, Git и архивы релиза.
- Ничего не отправляется провайдеру без явной отправки сообщения.
- Контекст документа можно отключить в настройках.

### Сборка из исходников

Требуется Rust 1.90 или новее и системные зависимости GPUI.

```bash
git clone https://github.com/device-vv/aircode.git
cd aircode
cargo run -p aircode-app
```

Проверка:

```bash
cargo fmt --all -- --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo build --release --target x86_64-pc-windows-msvc
```

### Горячие клавиши

Используйте `Ctrl` в Windows/Linux и `Cmd` в macOS.

| Действие | Сочетание |
| --- | --- |
| Открыть файл | `Ctrl/Cmd + O` |
| Сохранить файл | `Ctrl/Cmd + S` |
| Палитра команд | `Ctrl/Cmd + P` |
| Отправить сообщение | `Ctrl/Cmd + Enter` |
| Новый чат | `Ctrl/Cmd + Shift + N` |
| Панель ИИ | `Ctrl/Cmd + Shift + I` |
| Focus mode | `Ctrl/Cmd + Shift + F` |
| Тема | `Ctrl/Cmd + Shift + T` |
| Настройки провайдера | `Ctrl/Cmd + ,` |
| Закрыть панель | `Esc` |

### Структура проекта

```text
crates/aircode-ai       Провайдеры, ключи, настройки, HTTP и SSE
crates/aircode-app      Точка входа aircode.exe
crates/aircode-core     Состояние рабочего пространства
crates/aircode-editor   Unicode-документ, файлы, undo/redo
crates/aircode-ui       GPUI-редактор, ввод, чат и настройки
.github/workflows       CI и Windows-релизы
```

### Лицензия

Apache-2.0. См. [LICENSE](LICENSE).
