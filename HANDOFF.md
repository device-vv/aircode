# HANDOFF: голос + совместное редактирование в AIRcode

Документ для продолжения работы (например, в Kimi Code). Всё, что описано,
уже реализовано и проверено; внизу — что осталось.

## Состав решения

Репозиторий: `aircode-main/` (workspace, Rust + GPUI, версия 0.3.3).

| Крейт | Роль |
|---|---|
| `crates/aircode-collab` | Клиент collab: `session.rs` (Yjs/yrs-документ поверх WS), `voice.rs` (WebRTC PCMU, feature `voice`), `audio.rs` (cpal микрофон/динамик, ресэмплинг в 8 кГц), `sync.rs` (дифф-синхронизация буфера), `actor.rs` (поток с tokio current_thread, команды/события по каналам) |
| `crates/aircode-collab-relay` | Автономный релей (lib + bin `aircode-relay`). Эндпоинты `/ws/code/<room>` (бинарные Yjs-кадры, хранит снапшот + лог UPDATE'ов) и `/ws/video/<room>` (JSON-сигналинг: welcome/peer-joined/offer/answer/ice/ready/hangup/peer-left) |
| `crates/aircode-ui` | `workspace.rs`: действия `ToggleCollab` (Ctrl+Shift+L), `ToggleVoice` (Ctrl+Shift+V), диалог host/join `collab_dialog`, панель участников `collab_panel` внизу левой панели (список пиров с цветными точками + кнопки голоса) |

**Никакой зависимости от uchi-on-line/Django.** Cookie (`sessionid`) в клиенте
опциональна — только для совместимости со старым бэкендом.

## Сборка (Windows, GNU-тулчейн)

Self-contained rustup GNU не имеет `as.exe`, поэтому в PATH первым — winlibs:

```
cmd //c "set PATH=<workspace>\tools\mingw64\bin;C:\Users\Roman\.cargo\bin;C:\Windows\System32;C:\Windows && cargo build -p aircode-app 2>&1"
```

Бинарник: `aircode-main/target/debug/aircode.exe` (debug ~600 МБ, собирается
несколько минут из-за gpui/webrtc; повторные сборки инкрементальны).

## Запуск совместной сессии

Через диалог (основной путь, env не нужны):

- Ctrl+Shift+L → диалог «Совместная сессия»: режим **Создать** (host) или
  **Присоединиться** (join), поля «Ваше имя», «Комната», в join-режиме —
  «Адрес друга». Введённое сохраняется в настройки (`collab_name/server/room`
  в `AppSettings`) и предзаполняется при следующем запуске.
- Host: режим «Создать» → Старт. Релей поднимается встроенно на
  `0.0.0.0:9415`, в статус-баре появляется адрес вида
  `ws://192.168.x.x:9415` + комната — его сообщаешь другу.
- Гость: режим «Присоединиться» → вводишь адрес друга (схему `ws://` можно
  опустить — допишется сама) → Старт.
- Голос: Ctrl+Shift+V (или кнопка в панели участников) — вход/выход, мьют
  микрофона — кнопка там же.
- Отдельный релей на выделенной машине: `aircode-relay.exe --bind 0.0.0.0:9415`.

Env-переменные (`AIRCODE_COLLAB_HOST/SERVER/ROOM`) по-прежнему работают как
предзаполнение полей диалога; `AIRCODE_COLLAB_SESSION` (cookie) нужна только
для релеев с Django-авторизацией.

## Проверки (все зелёные на момент хендоффа)

- `cargo test -p aircode-collab --features voice` — 13/13 (g711, диффы,
  UTF-16 регрессии, ресэмплер).
- `cargo test -p aircode-collab-relay` — 2/2; `-p aircode-editor` — 12/12;
  `-p aircode-i18n` — 6/6; `-p aircode-ui` — 82/82.
- e2e-примеры (против `aircode-relay --bind 127.0.0.1:9415`):
  - `examples/actor_smoke.rs` — актор ↔ релей, маркер правки;
  - `examples/relay_smoke.rs` — сессия + снапшот;
  - `examples/voice_pair_smoke.rs` (feature voice) — звонок Rust↔Rust:
    RtcConnected у обоих, 254 кадра звука прошли (`VOICE-PAIR:PASS`).
- JS-тест сигналинга: `prototype/js_client/video_signal_test.mjs`
  (`TEST:PASS`: welcome, адресный offer, broadcast ready, ping/pong, peer-left).

## Критические грабли (не наступать снова)

1. **yrs `OffsetKind::Bytes` по умолчанию — индексы в БАЙТАХ.** Документ
   создаётся с `OffsetKind::Utf16` (как у Yjs в браузере), конверсия
   байты→юниты — `bytes_to_utf16_units` в sync.rs. Иначе правки «уезжают»
   в середину строк на кириллице + паника yrs (subtract overflow).
2. Сеть нельзя крутить в gpui-рантайме (нет tokio reactor) — вся сеть в
   потоке актора (`actor.rs`), UI общается каналами + pump-задача 200 мс.
3. yrs `Awareness` и `Subscription` — !Send; live-awareness применяется в
   `next_event()` на потоке сессии.
4. Git Bash съедает `//` в CLI-аргументах (`// текст` → `/ текст`).
5. cpal-колбэки реального времени: никаких локов с сетью внутри, только
   короткие Mutex<VecDeque>.

## Что осталось (по приоритету)

1. Курсоры/выделения участников в редакторе из awareness (данные уже
   приходят в `CollabEvent::PeersChanged`, нужна отрисовка в DocumentView).
2. Голос mesh >2 участников (сейчас 1-на-1; протокол релея уже умеет `to`).
3. Реконнект при обрыве WS + индикатор сети в панели.
4. Индикатор «говорит» (уровень PCM уже доступен в каналах audio.rs).
5. Release-сборка под msvc (опционально; GNU собирает и так, VS C++
   workload не обязателен).
6. Persist истории комнаты на хосте (сейчас снапшот+лог в памяти релея).
