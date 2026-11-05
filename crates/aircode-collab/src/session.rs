//! Клиент совместного редактирования `/ws/code/<room_id>/`.
//!
//! Поведение зеркалит `frontend/codepad/src/transport.js` (YRelay):
//! при подключении шлём SYNC_REQUEST + полный апдейт (если есть) + awareness,
//! локальные правки уходят UPDATE-кадрами, снапшот — по запросу/таймеру.

use std::sync::{Arc, Mutex};

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::http::Request;
use tokio_tungstenite::tungstenite::Message;
use yrs::sync::{Awareness, AwarenessUpdate};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Doc, GetString, ReadTxn, StateVector, Text, Transact, Update};

use crate::protocol::*;

/// Имя общего текста в Yjs-документе (как `doc.getText("code")` в кодпаде).
pub const TEXT_KEY: &str = "code";

/// Участник сессии (из awareness-протокола y-protocols).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerInfo {
    /// Yjs clientID участника.
    pub client_id: u64,
    /// Сырой JSON состояния (`{"user":{"name":...,"color":...}}` в кодпаде).
    pub state_json: Option<String>,
}

/// События сессии для UI.
#[derive(Debug, Clone)]
pub enum CollabEvent {
    /// WebSocket подключён, приветственный обмен отправлен.
    Connected,
    /// Документ синхронизирован (получен STATE снимок или SYNC_REPLY).
    Synced,
    /// Текст изменился удалённой стороной; новое содержимое — [`CollabSession::text`].
    RemoteTextChanged,
    /// Состав участников изменился.
    PeersChanged(Vec<PeerInfo>),
    /// Соединение закрыто (сервер или сеть). Коды 4001/4003 — ошибки доступа.
    Closed(String),
}

/// Сессия совместного редактирования одного документа в комнате.
///
/// **Не `Send`** (awareness yrs держит не-потокобезопасные колбэки):
/// живите с сессией на одном потоке, что для GPUI-приложения естественно.
pub struct CollabSession {
    doc: Arc<Mutex<Doc>>,
    awareness: Awareness,
    out_tx: mpsc::UnboundedSender<Message>,
    events_rx: mpsc::UnboundedReceiver<CollabEvent>,
    // Сырые awareness-кадры из сетевой задачи; применяются в next_event,
    // т.к. Awareness не Send и жить в tokio::spawn не может.
    awareness_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    net_task: JoinHandle<()>,
    writer_task: JoinHandle<()>,
    // Подписка на локальные правки; не Send, поэтому живёт здесь,
    // а не в сетевой задаче. Пока поле живо — жив и наблюдатель.
    _update_sub: yrs::Subscription,
}

impl CollabSession {
    /// Подключиться к релею комнаты.
    ///
    /// * `ws_url` — например `ws://host:9415/ws/code/<room>/`;
    /// * `session_cookie` — опционально: значение cookie `sessionid`, если
    ///   релей требует Django-авторизацию (режим совместимости). Для
    ///   автономного `aircode-collab-relay` — `None`;
    /// * `user_name` — имя для awareness.
    pub async fn connect(
        ws_url: &str,
        session_cookie: Option<&str>,
        user_name: &str,
    ) -> anyhow::Result<Self> {
        let mut builder = Request::builder()
            .uri(ws_url)
            .header("Host", ws_url.split('/').nth(2).unwrap_or("localhost"))
            .header("Connection", "Upgrade")
            .header("Upgrade", "websocket")
            .header("Sec-WebSocket-Version", "13")
            .header(
                "Sec-WebSocket-Key",
                tokio_tungstenite::tungstenite::handshake::client::generate_key(),
            );
        if let Some(cookie) = session_cookie {
            builder = builder.header("Cookie", format!("sessionid={cookie}"));
        }
        let request = builder.body(())?;
        let (ws, _resp) = tokio_tungstenite::connect_async(request).await?;
        let (mut ws_tx, mut ws_rx) = ws.split();

        let (out_tx, mut out_rx) = mpsc::unbounded_channel::<Message>();
        let writer_task = tokio::spawn(async move {
            while let Some(m) = out_rx.recv().await {
                if ws_tx.send(m).await.is_err() {
                    break;
                }
            }
        });

        let (events_tx, events_rx) = mpsc::unbounded_channel::<CollabEvent>();

        // ВАЖНО: offset_kind Utf16 — иначе yrs считает индексы в байтах,
        // а Yjs (браузерный кодпад) — в UTF-16 code units. Смешение режимов
        // не ломает сам CRDT (правки интегрируются по границам блоков), но
        // точка вставки/удаления «уезжает» на не-ASCII тексте.
        let doc = Doc::with_options(yrs::Options {
            offset_kind: yrs::OffsetKind::Utf16,
            ..Default::default()
        });
        // Локальные правки -> UPDATE-кадры.
        let (upd_tx, mut upd_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let update_sub = {
            let upd_tx = upd_tx.clone();
            doc.observe_update_v1(move |_txn, e| {
                let _ = upd_tx.send(e.update.to_vec());
            })?
        };

        let doc = Arc::new(Mutex::new(doc));
        let awareness = Awareness::new({
            let d = doc.lock().unwrap();
            d.clone()
        });

        // Представимся (как main.js: поле user {name, color, colorLight}).
        let name_json = user_name.replace('\\', "\\\\").replace('"', "\\\"");
        awareness.set_local_state_raw(format!(
            r##"{{"user":{{"name":"{name_json}","color":"#30bced","colorLight":"#30bced33"}}}}"##
        ));

        // Приветственный обмен (как YRelay.onopen).
        {
            let d = doc.lock().unwrap();
            let txn = d.transact();
            let sv = txn.state_vector().encode_v1();
            let _ = out_tx.send(Message::Binary(encode_frame(MSG_SYNC_REQUEST, &sv).into()));
            let full = txn.encode_state_as_update_v1(&StateVector::default());
            if full.len() > 2 {
                let _ = out_tx.send(Message::Binary(encode_frame(MSG_UPDATE, &full).into()));
            }
        }
        if let Ok(aw) = awareness.update() {
            let _ = out_tx.send(Message::Binary(encode_frame(MSG_AWARENESS, &aw.encode_v1()).into()));
        }
        let _ = events_tx.send(CollabEvent::Connected);

        let (awareness_tx, awareness_rx) = mpsc::unbounded_channel::<Vec<u8>>();

        // Сетевой цикл.
        let net_task = {
            let doc = doc.clone();
            let out_tx = out_tx.clone();
            let events_tx = events_tx.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        Some(update) = upd_rx.recv() => {
                            let _ = out_tx.send(Message::Binary(encode_frame(MSG_UPDATE, &update).into()));
                        }
                        msg = ws_rx.next() => {
                            let Some(msg) = msg else { break };
                            match msg {
                                Ok(Message::Binary(data)) => {
                                    let Some((kind, payload)) = decode_frame(&data) else { continue };
                                    match kind {
                                        MSG_STATE | MSG_UPDATE | MSG_SYNC_REPLY => {
                                            let Ok(u) = Update::decode_v1(payload) else { continue };
                                            {
                                                let d = doc.lock().unwrap();
                                                let mut txn = d.transact_mut();
                                                let _ = txn.apply_update(u);
                                            }
                                            let _ = events_tx.send(CollabEvent::RemoteTextChanged);
                                            if kind != MSG_UPDATE {
                                                let _ = events_tx.send(CollabEvent::Synced);
                                            }
                                        }
                                        MSG_SYNC_REQUEST => {
                                            let reply = {
                                                let d = doc.lock().unwrap();
                                                let txn = d.transact();
                                                StateVector::decode_v1(payload)
                                                    .ok()
                                                    .map(|sv| txn.encode_state_as_update_v1(&sv))
                                            };
                                            if let Some(diff) = reply {
                                                let _ = out_tx.send(Message::Binary(
                                                    encode_frame(MSG_SYNC_REPLY, &diff).into(),
                                                ));
                                            }
                                        }
                                        MSG_AWARENESS => {
                                            // Awareness не Send — применяем на стороне владельца сессии.
                                            let _ = awareness_tx.send(payload.to_vec());
                                        }
                                        _ => {}
                                    }
                                }
                                Ok(Message::Close(c)) => {
                                    let reason = c
                                        .map(|f| format!("{} {}", f.code, f.reason))
                                        .unwrap_or_else(|| "no reason".into());
                                    let _ = events_tx.send(CollabEvent::Closed(reason));
                                    break;
                                }
                                Ok(_) => {}
                                Err(e) => {
                                    let _ = events_tx.send(CollabEvent::Closed(format!("error: {e}")));
                                    break;
                                }
                            }
                        }
                    }
                }
            })
        };

        Ok(Self {
            doc,
            awareness,
            out_tx,
            events_rx,
            awareness_rx,
            net_task,
            writer_task,
            _update_sub: update_sub,
        })
    }

    /// Текущее содержимое общего документа (`code`).
    pub fn text(&self) -> String {
        let d = self.doc.lock().unwrap();
        let text = d.get_or_insert_text(TEXT_KEY);
        text.get_string(&d.transact())
    }

    /// Вставить текст в позицию `pos` (в UTF-16 code units, как у Yjs).
    pub fn insert(&self, pos: u32, s: &str) {
        let d = self.doc.lock().unwrap();
        let text = d.get_or_insert_text(TEXT_KEY);
        let mut txn = d.transact_mut();
        let len = text.len(&txn);
        text.insert(&mut txn, pos.min(len), s);
    }

    /// Удалить `len` UTF-16 code units начиная с позиции `pos`.
    pub fn remove(&self, pos: u32, len: u32) {
        let d = self.doc.lock().unwrap();
        let text = d.get_or_insert_text(TEXT_KEY);
        let mut txn = d.transact_mut();
        let cur = text.len(&txn);
        if pos < cur && len > 0 {
            text.remove_range(&mut txn, pos, len.min(cur - pos));
        }
    }

    /// Заменить всё содержимое (удобно при открытии файла в пустой комнате).
    pub fn replace_all(&self, s: &str) {
        let d = self.doc.lock().unwrap();
        let text = d.get_or_insert_text(TEXT_KEY);
        let mut txn = d.transact_mut();
        let len = text.len(&txn);
        if len > 0 {
            text.remove_range(&mut txn, 0, len);
        }
        text.insert(&mut txn, 0, s);
    }

    /// Отправить полный снапшот на сервер (как `YRelay.flushSnapshot`).
    pub fn flush_snapshot(&self) {
        let d = self.doc.lock().unwrap();
        let snap = d.transact().encode_state_as_update_v1(&StateVector::default());
        if !snap.is_empty() && snap.len() <= SNAPSHOT_SOFT_LIMIT {
            let _ = self
                .out_tx
                .send(Message::Binary(encode_frame(MSG_SNAPSHOT, &snap).into()));
        }
    }

    /// Текущий список участников по awareness.
    pub fn peers(&self) -> Vec<PeerInfo> {
        self.awareness
            .iter()
            .map(|(id, s)| PeerInfo {
                client_id: id,
                state_json: s.data.map(|d| d.to_string()),
            })
            .collect()
    }

    /// Следующее событие сессии (для UI-цикла).
    ///
    /// Здесь же применяются входящие awareness-кадры (см. поле `awareness_rx`).
    pub async fn next_event(&mut self) -> Option<CollabEvent> {
        loop {
            tokio::select! {
                ev = self.events_rx.recv() => return ev,
                raw = self.awareness_rx.recv() => {
                    let payload = raw?;
                    if let Ok(u) = AwarenessUpdate::decode_v1(&payload) {
                        if self.awareness.apply_update(u).is_ok() {
                            return Some(CollabEvent::PeersChanged(self.peers()));
                        }
                    }
                }
            }
        }
    }

    /// Корректно завершить сессию: снапшот, закрытие сокета, остановка задач.
    pub async fn close(self) {
        self.flush_snapshot();
        let _ = self.out_tx.send(Message::Close(None));
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        self.net_task.abort();
        self.writer_task.abort();
    }
}
