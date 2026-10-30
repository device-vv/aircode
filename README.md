# AIRcode

**A lightweight native code editor and AI workspace written in Rust.**
**Лёгкий нативный редактор кода и ИИ-рабочее пространство на Rust.**

[English](#english) · [Русский](#русский)

![AIRcode project workspace](docs/assets/aircode-prototype.png)

## English

AIRcode combines a native GPUI editor, a project explorer, Tree-sitter syntax highlighting, and direct streaming AI integrations. Version **0.3.3** provides unsigned portable builds for Windows x86_64, Linux x86_64, and macOS arm64.

No Electron, embedded browser, CLI agent, or protocol-translating proxy is used. API keys stay in the operating system's credential store.

### Download v0.3.3

Download from the [v0.3.3 release](https://github.com/device-vv/aircode/releases/tag/v0.3.3):

- `aircode-windows-x86_64.zip` — extract and run `aircode.exe`.
- `aircode-linux-x86_64.tar.gz` — extract and run `./aircode` on a Linux x86_64 desktop with the same fontconfig, FreeType, Wayland, and X11 libraries used by CI.
- `aircode-macos-arm64.tar.gz` — extract and run `./aircode` on Apple-silicon macOS.

Each archive has a `.sha256` file. The builds are **unsigned** and portable; Rust is not required. Windows SmartScreen may ask for confirmation. The release workflow builds and tests the Linux and macOS archives, but their graphical runtime has not been manually verified for 0.3.3.

### Features in v0.3.3

- Open a complete project folder with a lazy file tree and filesystem change watching.
- Open, edit, and save UTF-8 files, with Unicode selection, Windows IME input, clipboard, undo/redo, and unsaved-change confirmation.
- Tree-sitter highlighting for Rust, Python, Dart/Flutter, JavaScript, TypeScript, Go, C, C++, Java, C#, HTML, CSS, JSON, TOML, YAML, and Markdown. Only visible lines are rendered. Project markers distinguish Rust, Python, Dart, and Flutter projects; syntax highlighting does not imply LSP support.
- File tabs and an integrated project terminal. The terminal uses the system shell; use Ctrl+Shift+V (Command+V on macOS) to paste.
- Optional AI inline completion: enable it in AI settings, press Tab to accept a suggestion or Escape to dismiss it. When enabled, code around the cursor is sent to the selected provider and may incur charges.
- Chat and draft are saved per project. Clearing a conversation requires confirmation. Large conversations may be sent to the configured provider for automatic summarization; the visible history remains available.
- Named provider profiles can be attached to a project for explicit multi-agent assignments or leader-planned shared tasks. Agents run sequentially with the existing permission controls, and visible chats can be exported as Markdown snapshots.
- Project, editor, and agent panels with draggable splitters and saved widths.
- Custom AIRcode title bar and application icon; working minimize, maximize/restore, and close controls. No console window on Windows.
- Non-blocking file/folder dialogs and title-bar dragging across monitors, with repeated Windows window-state stress validation.
- English and Russian interfaces. English is the default; Russian-system users are offered a one-time language switch.
- Streaming AI chat with collapsible reasoning, cancellation, message copying, and context menus for cut/copy/paste/select all.
- Optional document or selection context, limited to 200 KB per request.
- Project agent tools: list, read, search, write, create directories, move, delete, and run commands. Long turns retain the original user goal and system instructions while bounding complete tool exchanges, so omitted context is explicitly marked without orphaning tool results.
- Agent commands now default to a 120-second timeout (bounded to 1–600 seconds), drain and cap output, stop the process tree on cancellation/timeout, use normalized Windows paths, and run without console windows. Commands are still never executed until the active permission mode permits them.
- The project tree and editor show the current AI file, highlighted parent folders, a compact **AI** badge, and completed-write markers without stealing focus. Proposed write content is shown only after the complete tool call and is clearly labelled as not written.
- Successful agent writes refresh clean open tabs, including inactive tabs. Dirty user buffers are preserved and reported as conflicts instead of being replaced.
- AI streams distinguish normal completion from token-limit, content-filter, error, truncated, and stalled responses. Bounded transient retries occur only before any text or tool action has been exposed.
- Three permission modes: **Ask every action**, **Ask important only**, and **Full access**. Approval includes bounded arguments, proposed write content, and a diff. An activity log records commands, tool calls, and bounded output.
- Light and dark themes, native Kimi Code support, and custom OpenAI-compatible providers.

Not included yet: LSP, debugging/DAP, Git operations, signed installers, and an OS-level command sandbox. Files larger than 16 MiB are rejected. Linux and macOS packages are built by CI but are not manually runtime-validated for this release.

### Agent permissions and safety

- **Ask every action** requires approval for every tool, including read-only operations.
- **Ask important only** allows list/read/search but asks before writes, deletes, moves, directory creation, and commands.
- **Full access** executes all tools without confirmation. Use it only for a project and provider you trust.

File tools are restricted to the opened project. They reject parent-directory escapes and protected paths such as `.git`, `.env*`, `*.pem`, `*.key`, and credential files. Existing symlinks are not followed by listing and search.

**Shell commands are not sandboxed.** They run with your user permissions in the project root and can access anything available to that user, including protected paths. Commands have a 120-second default timeout (at most 600 seconds), bounded output, and process-tree cleanup on stop or timeout; this is not a substitute for an OS process-isolation sandbox. Agent commands run in isolated pipes and appear in the labelled agent activity view, not in your interactive terminal.

A project can also be opened at startup:

```powershell
aircode.exe --project "C:\path\to\project"
```

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

K3.1 Pro always uses thinking. `low`, `high`, and `max` reasoning effort levels are available; `high` is the default. Changing the provider, model, or effort stops an active turn but preserves the visible chat. Model availability and context limits depend on the membership tier.

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

The integration uses native HTTP/SSE with an `AIRcode/<version>` User-Agent. Provider and tool-call contract tests use local mock servers. Live access depends on your account and key; a real paid provider account was not used for release validation.

### Agent queues and chat export

In AI settings, save the current provider configuration under a unique alias such as `@deepseek` or `@kimi`, then attach the profiles to the current project. Up to eight profiles are stored without API keys; keys remain in the OS credential store. The composer shows chips for attached aliases.

Assign work explicitly with one mention per line:

```text
@deepseek implement X
@kimi review/test X
```

Alternatively, enable **Shared task**, mention two or more agents, and enter the common task. The first mentioned agent is the leader. With tools disabled, it must return a bounded JSON plan containing exactly one nonempty task for each selected alias. Invalid plans, unknown aliases, missing models, and missing keys stop before execution with an actionable error.

Queue tasks use saved provider snapshots and run one at a time in the same project root. Only successful earlier results are shared, with their source aliases labelled. Every task uses the normal approval, permission, and cancellation flow. Stop cancels the active task and the whole pending queue; individual pending tasks can be skipped. Queue state is not resumed after restart.

**Export chat...** opens a native save dialog in the project root with `aircode-chat.md` suggested. The Markdown file contains the visible conversation, agent labels, Unicode and original code blocks, plus partial, failed, or cancelled states. Hidden reasoning and raw tool arguments are excluded. Export does not replace or import the separate resumable internal history.

### Security

- API keys are saved in Windows Credential Manager or the equivalent OS backend.
- Settings contain only provider, interface, panel, permission, and context preferences.
- Keys are excluded from logs, errors, Git, and release archives.
- Data is sent when you send a message, when a large project history is summarized, or when you enable AI inline completion and type in the editor.
- Document context can be disabled in Settings.

### Build from source

For an isolated local test, set `AIRCODE_DATA_DIR` to an absolute directory before launching AIRcode. Settings and internal project histories use that directory; API keys still use the OS credential store. Markdown exports are ordinary user-selected files and can contain project code or private conversation text.

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
| Save file as | `Ctrl/Cmd + Shift + S` |
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
crates/aircode-agent     Permission modes, sandboxed file tools, commands
crates/aircode-ai        Providers, credentials, settings, HTTP and SSE
crates/aircode-app       aircode.exe entry point and Windows resources
crates/aircode-core      Workspace state
crates/aircode-editor    Unicode document, file I/O, undo/redo
crates/aircode-i18n      English/Russian translations and locale detection
crates/aircode-project   Lazy project tree and filesystem watcher
crates/aircode-syntax    Incremental Tree-sitter parsing and highlights
crates/aircode-ui        GPUI editor, panels, chat and settings
.github/workflows        CI and native portable releases
```

### License

Apache-2.0. See [LICENSE](LICENSE).

---

## Русский

AIRcode объединяет нативный GPUI-редактор, дерево проекта, подсветку Tree-sitter и прямые потоковые ИИ-интеграции. Версия **0.3.3** — неподписанные portable-сборки для Windows x86_64, Linux x86_64 и macOS arm64.

Без Electron, встроенного браузера, CLI-агентов и прокси для преобразования протоколов. API-ключи хранятся в системном хранилище ОС.

### Скачать v0.3.3

Скачайте файлы из [релиза v0.3.3](https://github.com/device-vv/aircode/releases/tag/v0.3.3):

- `aircode-windows-x86_64.zip` — распакуйте и запустите `aircode.exe`.
- `aircode-linux-x86_64.tar.gz` — распакуйте и запустите `./aircode` в Linux x86_64 с библиотеками fontconfig, FreeType, Wayland и X11, которые использует CI.
- `aircode-macos-arm64.tar.gz` — распакуйте и запустите `./aircode` на macOS с Apple Silicon.

Рядом с каждым архивом публикуется `.sha256`. Сборки **не подписаны**, portable, Rust не требуется. Windows SmartScreen может запросить подтверждение. Linux- и macOS-архивы собираются и тестируются в CI, но графический запуск этих сборок вручную для 0.3.3 не проверялся.

### Возможности v0.3.3

- Открытие всей папки проекта, ленивое дерево файлов и отслеживание изменений на диске.
- Открытие, редактирование и сохранение UTF-8 файлов, Unicode-выделение, Windows IME, буфер обмена, undo/redo и подтверждение при потере изменений.
- Подсветка Tree-sitter для Rust, Python, Dart/Flutter, JavaScript, TypeScript, Go, C, C++, Java, C#, HTML, CSS, JSON, TOML, YAML и Markdown. Отрисовываются только видимые строки. Маркеры проекта различают Rust, Python, Dart и Flutter; наличие подсветки не означает поддержку LSP.
- Вкладки файлов и встроенный терминал проекта на системной оболочке. Вставка: Ctrl+Shift+V, на macOS — Command+V.
- Подсказки кода ИИ можно включить в настройках: Tab принимает подсказку, Escape отменяет её. Код вокруг курсора отправляется выбранному провайдеру; запросы могут быть платными.
- Чат и черновик сохраняются отдельно для каждого проекта. Полная очистка требует подтверждения. Большая переписка может автоматически отправляться провайдеру для сжатия; видимая история остаётся доступной.
- Именованные профили провайдеров можно подключать к проекту для явных заданий нескольким агентам или общей задачи с планом лидера. Агенты работают последовательно с прежними разрешениями, а видимый чат экспортируется в Markdown.
- Панели проекта, редактора и агента с изменяемой шириной и сохранением размеров.
- Фирменная строка заголовка и иконка, рабочие сворачивание, разворачивание/восстановление и закрытие. Консоль Windows не открывается.
- Неблокирующие диалоги файлов/папок и перетаскивание окна между мониторами; повторные стресс-проверки состояния окна Windows.
- Английский и русский интерфейсы. По умолчанию английский; при русской системе предлагается однократное переключение.
- Потоковый чат, сворачиваемые рассуждения, остановка, копирование сообщений и контекстное меню вырезания/копирования/вставки/выделения.
- Контекст документа или выделения по выбору пользователя, до 200 КБ на запрос.
- Инструменты агента: список, чтение, поиск, запись, создание папок, перемещение, удаление и команды. В длинных сессиях сохраняются исходная цель пользователя и системные инструкции, а полные обмены инструментов ограничиваются без «осиротевших» результатов; пропуск контекста явно помечается.
- Команды агента по умолчанию имеют тайм-аут 120 секунд (1–600 секунд), ограниченный буфер вывода, остановку дерева процессов при отмене/тайм-ауте, нормализованные пути Windows и запуск без консольных окон. Команда не выполняется, пока этого не разрешит режим доступа.
- Дерево проекта и редактор показывают текущий файл ИИ, подсвеченные родительские папки, компактный бейдж **ИИ** и отметки завершённой записи, не забирая фокус. Предлагаемое содержимое показывается только после полного вызова и явно помечается как незаписанное.
- После успешной записи агента чистые открытые вкладки, включая неактивные, перезагружаются. Несохранённые буферы не перезаписываются и отображаются как конфликт.
- Потоки ИИ различают нормальное завершение, лимит токенов, content filter, ошибку, обрыв и зависание. Ограниченные повторные попытки выполняются только до появления текста или действия инструмента.
- Три режима: **Спрашивать каждое действие**, **Спрашивать только важное**, **Полный доступ**. Перед записью показываются ограниченные аргументы, предлагаемое содержимое и diff. Журнал содержит команды, вызовы и ограниченный вывод.
- Светлая и тёмная темы, нативный Kimi Code и пользовательские OpenAI-совместимые провайдеры.

Пока не входят: LSP, отладка/DAP, Git, подписанные установщики и песочница процессов ОС. Файлы больше 16 МиБ отклоняются. Linux- и macOS-пакеты собираются в CI, но их графический запуск вручную для этого релиза не проверялся.

### Разрешения и безопасность агента

- **Спрашивать каждое действие** требует подтверждения всех инструментов, включая чтение.
- **Спрашивать только важное** разрешает список/чтение/поиск, но спрашивает перед записью, удалением, перемещением, созданием папок и командами.
- **Полный доступ** выполняет всё без подтверждения. Используйте его только для доверенных проекта и провайдера.

Файловые инструменты ограничены открытым проектом и запрещают выход через `..`, а также `.git`, `.env*`, `*.pem`, `*.key` и credential-файлы. Список и поиск не переходят по существующим символическим ссылкам.

**Shell-команды не изолированы.** Они работают с правами пользователя в корне проекта и могут обращаться ко всем доступным ему данным, включая защищённые пути. По умолчанию тайм-аут 120 секунд (не более 600), вывод ограничен, а при остановке или тайм-ауте завершается дерево процессов. Это не заменяет изоляцию процессов ОС. Команды агента работают через отдельные каналы и отображаются в помеченном журнале агента, а не во встроенном интерактивном терминале.

Папку можно открыть при запуске:

```powershell
aircode.exe --project "C:\path\to\project"
```

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

K3.1 Pro всегда использует thinking. Доступны `low`, `high` и `max`, по умолчанию — `high`. При смене провайдера, модели или effort активный ответ останавливается, а видимая переписка сохраняется. Доступность моделей и лимит контекста зависят от подписки.

Настройка:

1. Создайте membership API key в Kimi Code Console.
2. Откройте **Настройки** в AIRcode.
3. Выберите **Kimi Code**, регион и `k3.1-pro`.
4. Вставьте ключ и нажмите **Сохранить ключ**.
5. Закройте настройки, напишите сообщение и нажмите **Отправить** или `Ctrl/Cmd + Enter`.

Отдельный профиль **Kimi Platform** поддерживает официальные Moonshot endpoint'ы с ручным вводом model ID. Идентификаторы Coding Plan автоматически не применяются к Platform.

Официальные источники: [обзор Kimi Code](https://www.kimi.com/code/docs/en/) и [настройка моделей](https://www.kimi.com/code/docs/en/kimi-code/models.html).

### Другие провайдеры

- **DeepSeek** с официальным preset.
- **RouterAI** с `https://routerai.ru/api/v1` и идентификаторами `provider/model`.
- **OpenAI-совместимые** сервисы с пользовательскими `base_url` и model ID.

Используется нативный HTTP/SSE и `AIRcode/<version>` User-Agent. Контрактные тесты работают с локальными mock-серверами. Доступ зависит от аккаунта и ключа; реальный платный аккаунт для проверки релиза не использовался.

### Очереди агентов и экспорт чата

В настройках ИИ сохраните конфигурацию провайдера под уникальным псевдонимом, например `@deepseek` или `@kimi`, и подключите профили к текущему проекту. Хранится не более восьми профилей без API-ключей; ключи остаются в хранилище учётных данных ОС. Над полем ввода показываются псевдонимы подключённых агентов.

Явные задания записываются по одному на строку:

```text
@deepseek implement X
@kimi review/test X
```

Либо включите **Общая задача**, упомяните двух или более агентов и введите общую цель. Первый агент становится лидером. Без инструментов он должен вернуть ограниченный JSON-план ровно с одной непустой задачей для каждого выбранного псевдонима. Неверный план, неизвестный псевдоним, отсутствующая модель или ключ останавливают запуск с понятной ошибкой.

Задачи используют сохранённые снимки конфигурации и выполняются по одной в корне проекта. Последующим агентам передаются только успешные предыдущие результаты с указанием источника. Действуют обычные разрешения, подтверждения и отмена. **Стоп** отменяет активную задачу и всю очередь; ожидающие задачи можно пропустить. После перезапуска очередь не возобновляется.

**Экспорт чата…** открывает системный диалог сохранения в корне проекта с именем `aircode-chat.md`. Markdown содержит видимую переписку, имена агентов, Unicode и исходные блоки кода, а также неполные, ошибочные и отменённые состояния. Скрытые рассуждения и сырые аргументы инструментов исключены. Экспорт не заменяет отдельную возобновляемую внутреннюю историю.

### Безопасность

- API-ключи сохраняются в Windows Credential Manager или эквивалентном backend ОС.
- В настройках находятся только провайдер, интерфейс, панели, разрешения и контекст.
- Ключи не попадают в логи, ошибки, Git и архивы релиза.
- Данные отправляются провайдеру при отправке сообщения, при сжатии большой истории проекта или при вводе кода с включёнными подсказками ИИ.
- Контекст документа можно отключить в настройках.

### Сборка из исходников

Для отдельной тестовой конфигурации задайте `AIRCODE_DATA_DIR` с абсолютным путём перед запуском AIRcode. Настройки и внутренняя история проектов будут храниться там; API-ключи остаются в системном хранилище ОС. Markdown-экспорт — обычный выбранный вами файл: он может содержать код проекта и личную переписку.

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
| Сохранить как | `Ctrl/Cmd + Shift + S` |
| Палитра команд | `Ctrl/Cmd + P` |
| Отправить сообщение | `Ctrl/Cmd + Enter` |
| Новый чат | `Ctrl/Cmd + Shift + N` |
| Панель ИИ | `Ctrl/Cmd + Shift + I` |
| Режим фокуса | `Ctrl/Cmd + Shift + F` |
| Тема | `Ctrl/Cmd + Shift + T` |
| Настройки провайдера | `Ctrl/Cmd + ,` |
| Закрыть панель | `Esc` |

### Структура проекта

```text
crates/aircode-agent     Разрешения, файловые инструменты, команды
crates/aircode-ai        Провайдеры, ключи, настройки, HTTP и SSE
crates/aircode-app       Точка входа и ресурсы Windows
crates/aircode-core      Состояние рабочего пространства
crates/aircode-editor    Unicode-документ, файлы, undo/redo
crates/aircode-i18n      Английский/русский и определение локали
crates/aircode-project   Дерево проекта и отслеживание изменений
crates/aircode-syntax    Инкрементальный Tree-sitter и подсветка
crates/aircode-ui        GPUI-редактор, панели, чат и настройки
.github/workflows        CI и нативные portable-релизы
```

### Лицензия

Apache-2.0. См. [LICENSE](LICENSE).
