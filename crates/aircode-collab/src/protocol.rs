//! Кадровый протокол релея `/ws/code/<room_id>/`.
//!
//! Зеркалит `codepad/consumers.py` (сервер) и `frontend/codepad/src/transport.js`
//! (браузерный клиент) проекта uchi-on-line. Формат кадра: первый байт — тип,
//! остальное — полезная нагрузка (бинарные Yjs-апдейты).

/// Правка документа (Yjs update) — рассылается остальным участникам.
pub const MSG_UPDATE: u8 = 0;
/// Полный снимок — сохраняется на сервере, не рассылается.
pub const MSG_SNAPSHOT: u8 = 1;
/// Курсоры и имена (awareness) — рассылается остальным.
pub const MSG_AWARENESS: u8 = 2;
/// Вектор состояния нового клиента — рассылается остальным.
pub const MSG_SYNC_REQUEST: u8 = 3;
/// Ответ на [`MSG_SYNC_REQUEST`] — рассылается остальным.
pub const MSG_SYNC_REPLY: u8 = 4;
/// Сохранённый снимок: сервер -> только что подключившийся клиент.
pub const MSG_STATE: u8 = 5;

/// Лимиты сервера (`codepad/consumers.py`).
pub const MAX_FRAME_BYTES: usize = 1_000_000;
/// Клиентский лимит снапшота (как в `transport.js`: 900_000).
pub const SNAPSHOT_SOFT_LIMIT: usize = 900_000;

/// Собрать кадр: байт типа + полезная нагрузка.
pub fn encode_frame(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(payload.len() + 1);
    v.push(kind);
    v.extend_from_slice(payload);
    v
}

/// Разобрать кадр на (тип, полезная нагрузка).
pub fn decode_frame(frame: &[u8]) -> Option<(u8, &[u8])> {
    let (kind, payload) = frame.split_first()?;
    Some((*kind, payload))
}

/// Сигналинг `/ws/video/<room_id>/`: разрешённые типы сообщений
/// (`video/consumers.py`). Все сообщения — JSON.
pub mod signal {
    /// SDP offer: `{type, kind: "cam"|"screen", to, sdp: {type, sdp}}`.
    pub const OFFER: &str = "offer";
    /// SDP answer: `{type, kind, to, sdp: {type, sdp}}`.
    pub const ANSWER: &str = "answer";
    /// ICE-кандидат: `{type, kind, to, candidate: {candidate, sdpMid, sdpMLineIndex}}`.
    pub const ICE: &str = "ice";
    /// Завершение звонка: `{type}`.
    pub const HANGUP: &str = "hangup";
    /// Приглашение к установке связи: `{type, to?}`.
    pub const READY: &str = "ready";
    /// Состояние медиа: `{type, camera, microphone}`.
    pub const MEDIA_STATE: &str = "media-state";
    /// Сервер -> клиент: присвоенный `connection_id`.
    pub const WELCOME: &str = "welcome";
    /// Сервер -> все: в комнату вошёл участник `{user_id, connection_id, session}`.
    pub const PEER_JOINED: &str = "peer-joined";
    /// Сервер -> все: участник вышел.
    pub const PEER_LEFT: &str = "peer-left";
    /// Keepalive: `{type:"ping"}` -> `{type:"pong"}`.
    pub const PING: &str = "ping";

    /// Аудио+видео линия (голос в AIRcode идёт по ней, видео нет).
    pub const KIND_CAM: &str = "cam";
    /// Линия демонстрации экрана.
    pub const KIND_SCREEN: &str = "screen";
}
