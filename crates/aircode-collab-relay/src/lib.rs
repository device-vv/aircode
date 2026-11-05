//! Автономный релей совместной работы AIRcode — никаких внешних сервисов.
//!
//! Один участник поднимает релей у себя (встроен в AIRcode или отдельным
//! бинарником `aircode-relay`), остальные подключаются по адресу
//! `ws://<host>:<port>/ws/code/<комната>`. Комната — произвольная строка,
//! она же и есть «секрет» встречи: кто знает адрес и имя комнаты, тот внутри.
//!
//! Два эндпоинта (тот же проводной протокол, что умеет `aircode-collab`):
//!
//! * `/ws/code/<room>` — совместное редактирование. Бинарные кадры Yjs:
//!   первый байт — тип (0 UPDATE, 1 SNAPSHOT, 2 AWARENESS, 3 SYNC_REQUEST,
//!   4 SYNC_REPLY, 5 STATE). Релей хранит последний снимок комнаты и
//!   пересылает остальные кадры всем прочим участникам; сам документ не
//!   разбирает — конфликты разрешает CRDT на клиентах.
//!
//! * `/ws/video/<room>` — сигналинг голосовых звонков (WebRTC). JSON:
//!   `welcome`, `peer-joined`, `peer-left`, адресные `offer`/`answer`/`ice`
//!   (поле `to`), широковещательные `ready`/`hangup`/`media-state`.
//!   Медиа идёт peer-to-peer, через релей только сигналинг.

use std::collections::HashMap;
use std::io;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

/// Максимальный размер одного кадра (как в референс-протоколе).
const MAX_FRAME_BYTES: usize = 1_000_000;

// Типы бинарных кадров совместного редактирования.
const MSG_UPDATE: u8 = 0;
const MSG_SNAPSHOT: u8 = 1;
const MSG_AWARENESS: u8 = 2;
const MSG_SYNC_REQUEST: u8 = 3;
const MSG_SYNC_REPLY: u8 = 4;
const MSG_STATE: u8 = 5;

static ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Простой генератор 32-hex идентификаторов (xorshift от времени и
/// счётчика: криптостойкость не нужна, важна уникальность в пределах релея).
fn next_connection_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut x = (nanos ^ ID_COUNTER.fetch_add(1, Ordering::Relaxed).wrapping_mul(0x9E3779B97F4A7C15))
        | 1;
    let mut out = String::with_capacity(32);
    for _ in 0..2 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.push_str(&format!("{x:016x}"));
    }
    out
}

type PeerTx = mpsc::UnboundedSender<Message>;

#[derive(Default)]
struct CodeRoom {
    /// Последний снимок документа (полезная нагрузка без байта типа).
    snapshot: Option<Vec<u8>>,
    /// Лог UPDATE-кадров после снимка: Yjs-апдейты идемпотентны и
    /// комбинируются, поэтому новичку можно просто переиграть их по
    /// порядку — документ соберётся на клиенте (CRDT).
    updates: Vec<Vec<u8>>,
    /// Суммарный размер `updates` (для ограничения памяти).
    updates_bytes: usize,
    peers: HashMap<u64, PeerTx>,
}

/// Максимум памяти на лог правок одной комнаты (после — просим клиентов
/// прислать свежий снимок: старые апдейты вытесняются).
const MAX_UPDATES_LOG_BYTES: usize = 2_000_000;

#[derive(Default)]
struct VideoRoom {
    peers: HashMap<String, PeerTx>,
}

#[derive(Default)]
struct State {
    code: HashMap<String, CodeRoom>,
    video: HashMap<String, VideoRoom>,
}

/// Живой релей. Пока не дропнут — принимает подключения.
pub struct RelayHandle {
    addr: SocketAddr,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<thread::JoinHandle<()>>,
}

impl RelayHandle {
    /// Адрес, на котором слушает релей (полезно при порте 0).
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Поднять релей на отдельном потоке. `bind` — например `0.0.0.0:9415`
    /// (принимать подключения из сети) или `127.0.0.1:0` (эфемерный порт,
    /// только локально).
    pub fn spawn(bind: SocketAddr) -> io::Result<Self> {
        // Биндим синхронно: ошибки (порт занят) видны сразу, порт известен.
        let std_listener = std::net::TcpListener::bind(bind)?;
        std_listener.set_nonblocking(true)?;
        let addr = std_listener.local_addr()?;
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        let thread = thread::Builder::new()
            .name("aircode-relay".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("relay runtime");
                rt.block_on(async move {
                    let listener = TcpListener::from_std(std_listener).expect("relay listener");
                    let state = Arc::new(Mutex::new(State::default()));
                    loop {
                        tokio::select! {
                            _ = &mut shutdown_rx => break,
                            accepted = listener.accept() => {
                                match accepted {
                                    Ok((stream, _peer)) => {
                                        let state = Arc::clone(&state);
                                        tokio::spawn(async move {
                                            serve_connection(stream, state).await;
                                        });
                                    }
                                    Err(_) => break,
                                }
                            }
                        }
                    }
                });
            })?;
        Ok(Self {
            addr,
            shutdown: Some(shutdown_tx),
            thread: Some(thread),
        })
    }
}

impl Drop for RelayHandle {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        // Поток не джойним: он завершится по shutdown, а если клиент
        // повис — умрёт вместе с процессом.
        self.thread.take();
    }
}

/// Разобрать путь вида `/ws/code/<room>` или `/ws/video/<room>`.
fn parse_path(path: &str) -> Option<(&'static str, String)> {
    let rest = path.strip_prefix("/ws/")?;
    let (kind, room) = rest.split_once('/')?;
    let room = room.trim_matches('/');
    if room.is_empty() || room.len() > 128 {
        return None;
    }
    match kind {
        "code" => Some(("code", room.to_string())),
        "video" => Some(("video", room.to_string())),
        _ => None,
    }
}

async fn serve_connection(stream: TcpStream, state: Arc<Mutex<State>>) {
    // Захватываем путь запроса во время хендшейка.
    let path_slot = Arc::new(Mutex::new(String::new()));
    let path_slot_cb = Arc::clone(&path_slot);
    let ws = tokio_tungstenite::accept_hdr_async(
        stream,
        move |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
              response: tokio_tungstenite::tungstenite::handshake::server::Response| {
            *path_slot_cb.lock().unwrap() = request.uri().path().to_string();
            Ok(response)
        },
    )
    .await;
    let Ok(ws) = ws else { return };
    let path = path_slot.lock().unwrap().clone();
    match parse_path(&path) {
        Some(("code", room)) => serve_code_room(ws, state, room).await,
        Some(("video", room)) => serve_video_room(ws, state, room).await,
        _ => {} // неизвестный путь — просто закрываем
    }
}

async fn serve_code_room(
    ws: tokio_tungstenite::WebSocketStream<TcpStream>,
    state: Arc<Mutex<State>>,
    room: String,
) {
    let (mut sink, mut stream) = ws.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
    let peer_id = ID_COUNTER.fetch_add(1, Ordering::Relaxed);

    // Регистрация + нагон истории: снимок (если есть) + лог правок после него.
    {
        let mut guard = state.lock().unwrap();
        let room_state = guard.code.entry(room.clone()).or_default();
        room_state.peers.insert(peer_id, tx.clone());
        if let Some(snapshot) = &room_state.snapshot {
            let mut frame = Vec::with_capacity(1 + snapshot.len());
            frame.push(MSG_STATE);
            frame.extend_from_slice(snapshot);
            let _ = tx.send(Message::Binary(frame.into()));
        }
        for update in &room_state.updates {
            let _ = tx.send(Message::Binary(update.clone().into()));
        }
    }

    let writer = tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            if sink.send(message).await.is_err() {
                break;
            }
        }
    });

    while let Some(message) = stream.next().await {
        let Ok(message) = message else { break };
        let Message::Binary(data) = message else { continue };
        if data.is_empty() || data.len() > MAX_FRAME_BYTES {
            continue;
        }
        let kind = data[0];
        match kind {
            MSG_SNAPSHOT => {
                let mut guard = state.lock().unwrap();
                if let Some(room_state) = guard.code.get_mut(&room) {
                    room_state.snapshot = Some(data[1..].to_vec());
                    // Свежий снимок покрывает всю историю — лог можно сжать.
                    room_state.updates.clear();
                    room_state.updates_bytes = 0;
                }
            }
            MSG_UPDATE => {
                let mut guard = state.lock().unwrap();
                if let Some(room_state) = guard.code.get_mut(&room) {
                    // Копим для поздних участников; при переполнении —
                    // сбрасываем (поздний клиент тогда досинхронизируется
                    // SYNC_REQUEST'ом у живых пиров).
                    if room_state.updates_bytes + data.len() <= MAX_UPDATES_LOG_BYTES {
                        room_state.updates_bytes += data.len();
                        room_state.updates.push(data.to_vec());
                    } else {
                        room_state.updates.clear();
                        room_state.updates_bytes = 0;
                    }
                    for (id, peer) in &room_state.peers {
                        if *id != peer_id {
                            let _ = peer.send(Message::Binary(data.clone()));
                        }
                    }
                }
            }
            MSG_AWARENESS | MSG_SYNC_REQUEST | MSG_SYNC_REPLY => {
                let guard = state.lock().unwrap();
                if let Some(room_state) = guard.code.get(&room) {
                    for (id, peer) in &room_state.peers {
                        if *id != peer_id {
                            let _ = peer.send(Message::Binary(data.clone()));
                        }
                    }
                }
            }
            _ => {}
        }
    }

    state
        .lock()
        .unwrap()
        .code
        .get_mut(&room)
        .map(|room_state| room_state.peers.remove(&peer_id));
    writer.abort();
}

async fn serve_video_room(
    ws: tokio_tungstenite::WebSocketStream<TcpStream>,
    state: Arc<Mutex<State>>,
    room: String,
) {
    let (mut sink, mut stream) = ws.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Message>();
    let connection_id = next_connection_id();

    {
        let mut guard = state.lock().unwrap();
        let room_state = guard.video.entry(room.clone()).or_default();
        room_state.peers.insert(connection_id.clone(), tx.clone());
    }
    let _ = tx.send(Message::Text(
        serde_json::json!({"type": "welcome", "connection_id": connection_id})
            .to_string()
            .into(),
    ));
    broadcast_video(
        &state,
        &room,
        None,
        &serde_json::json!({"type": "peer-joined", "connection_id": connection_id}),
    );

    let writer = tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            if sink.send(message).await.is_err() {
                break;
            }
        }
    });

    const ALLOWED: [&str; 8] = [
        "offer",
        "answer",
        "ice",
        "hangup",
        "screen-on",
        "screen-off",
        "media-state",
        "ready",
    ];
    while let Some(message) = stream.next().await {
        let Ok(message) = message else { break };
        let Message::Text(text) = message else { continue };
        if text.len() > MAX_FRAME_BYTES {
            continue;
        }
        let Ok(mut content) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let Some(kind) = content.get("type").and_then(|t| t.as_str()) else {
            continue;
        };
        if kind == "ping" {
            let _ = tx.send(Message::Text(r#"{"type":"pong"}"#.into()));
            continue;
        }
        if !ALLOWED.contains(&kind) {
            continue;
        }
        content["connection_id"] = serde_json::Value::String(connection_id.clone());
        // Адресное сообщение — только адресату; без `to` — всем остальным.
        let target = content
            .get("to")
            .and_then(|t| t.as_str())
            .filter(|t| t.len() == 32 && t.chars().all(|c| c.is_ascii_hexdigit()))
            .map(|t| t.to_string());
        broadcast_video(&state, &room, target.as_deref(), &content);
    }

    {
        let mut guard = state.lock().unwrap();
        if let Some(room_state) = guard.video.get_mut(&room) {
            room_state.peers.remove(&connection_id);
        }
    }
    broadcast_video(
        &state,
        &room,
        None,
        &serde_json::json!({"type": "peer-left", "connection_id": connection_id}),
    );
    writer.abort();
}

/// Разослать JSON участникам видео-комнаты. `target` — конкретный
/// connection_id адресата; `None` — всем, кроме автора (автор уже вставил
/// свой connection_id в сообщение, а эхо ему не нужно).
fn broadcast_video(
    state: &Arc<Mutex<State>>,
    room: &str,
    target: Option<&str>,
    payload: &serde_json::Value,
) {
    let sender = payload
        .get("connection_id")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();
    let text = Message::Text(payload.to_string().into());
    let guard = state.lock().unwrap();
    if let Some(room_state) = guard.video.get(room) {
        for (id, peer) in &room_state.peers {
            match target {
                Some(target) if id != target => continue,
                None if *id == sender => continue,
                _ => {}
            }
            let _ = peer.send(text.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_paths() {
        assert_eq!(
            parse_path("/ws/code/main"),
            Some(("code", "main".to_string()))
        );
        assert_eq!(
            parse_path("/ws/video/abc-123/"),
            Some(("video", "abc-123".to_string()))
        );
        assert_eq!(parse_path("/ws/code/"), None);
        assert_eq!(parse_path("/other/code/x"), None);
        assert_eq!(parse_path("/ws/nope/x"), None);
    }

    #[test]
    fn connection_ids_are_unique_hex32() {
        let a = next_connection_id();
        let b = next_connection_id();
        assert_ne!(a, b);
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
