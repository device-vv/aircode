//! Актор сессии совместного редактирования для UI.
//!
//! [`CollabSession`] не `Send` (awareness yrs), поэтому сессия живёт на
//! выделенном потоке со своим current_thread tokio-рантаймом, а с UI
//! общается каналами: команды [`CollabCommand`] внутрь, события
//! [`CollabEvent`] наружу. Каналы tokio — `Send`, потокобезопасность
//! соблюдается без единого `unsafe`.

use std::thread;

use tokio::sync::mpsc;

use crate::session::{CollabEvent as SessionEvent, CollabSession, PeerInfo};
use crate::sync::CollabBinding;

/// Параметры подключения к комнате совместной работы.
#[derive(Debug, Clone)]
pub struct CollabConfig {
    /// Базовый URL релея, например `ws://192.168.1.5:9415`.
    pub server: String,
    /// Комната (произвольная строка; она же «секрет» встречи).
    pub room: String,
    /// Сессионная cookie — только если релей требует Django-авторизацию
    /// (режим совместимости); для автономного релея — `None`.
    pub session_cookie: Option<String>,
    /// Имя участника для awareness.
    pub user_name: String,
}

impl CollabConfig {
    /// WebSocket-URL релея совместного редактирования.
    pub fn code_ws_url(&self) -> String {
        format!(
            "{}/ws/code/{}/",
            self.server.trim_end_matches('/'),
            self.room
        )
    }

    /// WebSocket-URL сигналинга голосовых звонков.
    pub fn video_ws_url(&self) -> String {
        format!(
            "{}/ws/video/{}/",
            self.server.trim_end_matches('/'),
            self.room
        )
    }
}

/// Команды актору из UI.
#[derive(Debug)]
pub enum CollabCommand {
    /// Текущий текст буфера редактора целиком; актор сам посчитает дифф.
    LocalChange(String),
    /// Отправить полный снапшот на сервер (например, при сохранении файла).
    FlushSnapshot,
    /// Войти в голосовой канал комнаты (микрофон + динамик).
    #[cfg(feature = "voice")]
    StartVoice,
    /// Выйти из голосового канала.
    #[cfg(feature = "voice")]
    StopVoice,
    /// Выключить/включить микрофон.
    #[cfg(feature = "voice")]
    SetVoiceMuted(bool),
    /// Завершить сессию.
    Close,
}

/// События актора для UI.
#[derive(Debug, Clone)]
pub enum CollabEvent {
    /// WebSocket поднят, приветственный обмен отправлен.
    Started,
    /// Документ синхронизирован; полный текст сессии (применить к буферу).
    Synced(String),
    /// Удалённая сторона изменила текст; полный текст сессии.
    TextChanged(String),
    /// Состав участников изменился.
    PeersChanged(Vec<PeerInfo>),
    /// Голосовой канал: мы вошли (сигналинг поднят).
    #[cfg(feature = "voice")]
    VoiceStarted,
    /// Голосовой канал: собеседник появился (connection_id).
    #[cfg(feature = "voice")]
    VoicePeerJoined(String),
    /// Голосовой канал: собеседник вышел.
    #[cfg(feature = "voice")]
    VoicePeerLeft,
    /// Голосовой канал: WebRTC установлен, звук пошёл.
    #[cfg(feature = "voice")]
    VoiceRtcConnected,
    /// Голосовой канал закрыт (по команде или обрыву).
    #[cfg(feature = "voice")]
    VoiceStopped(String),
    /// Соединение закрыто (причина/код).
    Closed(String),
    /// Не удалось подключиться.
    Failed(String),
}

/// Живой актор сессии. Пока не дропнут — сессия работает.
pub struct CollabHandle {
    /// Команды актору (неблокирующая отправка с любого потока).
    pub cmd_tx: mpsc::UnboundedSender<CollabCommand>,
    /// События от актора (дренировать из UI-цикла через `try_recv`).
    pub event_rx: mpsc::UnboundedReceiver<CollabEvent>,
    thread: Option<thread::JoinHandle<()>>,
}

impl CollabHandle {
    /// Поднять актор: выделенный поток + current_thread рантайм + сессия.
    pub fn spawn(config: CollabConfig) -> Self {
        let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<CollabCommand>();
        let (event_tx, event_rx) = mpsc::unbounded_channel::<CollabEvent>();
        let thread = thread::Builder::new()
            .name("aircode-collab".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("collab runtime");
                rt.block_on(async move {
                    let url = config.code_ws_url();
                    let mut session = match CollabSession::connect(
                        &url,
                        config.session_cookie.as_deref(),
                        &config.user_name,
                    )
                    .await
                    {
                        Ok(session) => session,
                        Err(error) => {
                            let _ = event_tx.send(CollabEvent::Failed(error.to_string()));
                            return;
                        }
                    };
                    let _ = event_tx.send(CollabEvent::Started);
                    let mut binding = CollabBinding::new(String::new());
                    #[cfg(feature = "voice")]
                    let mut voice: Option<crate::voice::VoiceCall> = None;
                    #[cfg(feature = "voice")]
                    let mut voice_mic: Option<crate::audio::MicStream> = None;
                    #[cfg(feature = "voice")]
                    let mut _voice_speaker: Option<crate::audio::SpeakerStream> = None;
                    loop {
                        tokio::select! {
                            ev = session.next_event() => {
                                let Some(ev) = ev else { break };
                                match ev {
                                    SessionEvent::Connected => {}
                                    SessionEvent::Synced => {
                                        let text = session.text();
                                        binding.reset(text.clone());
                                        if event_tx.send(CollabEvent::Synced(text)).is_err() {
                                            break;
                                        }
                                    }
                                    SessionEvent::RemoteTextChanged => {
                                        let text = session.text();
                                        binding.reset(text.clone());
                                        if event_tx.send(CollabEvent::TextChanged(text)).is_err() {
                                            break;
                                        }
                                    }
                                    SessionEvent::PeersChanged(peers) => {
                                        if event_tx.send(CollabEvent::PeersChanged(peers)).is_err() {
                                            break;
                                        }
                                    }
                                    SessionEvent::Closed(reason) => {
                                        let _ = event_tx.send(CollabEvent::Closed(reason));
                                        break;
                                    }
                                }
                            }
                            cmd = cmd_rx.recv() => {
                                let Some(cmd) = cmd else { break };
                                match cmd {
                                    CollabCommand::LocalChange(text) => {
                                        if let Some((pos, delete, insert)) =
                                            binding.local_change(&text)
                                        {
                                            if delete > 0 {
                                                session.remove(pos, delete);
                                            }
                                            if !insert.is_empty() {
                                                session.insert(pos, &insert);
                                            }
                                        }
                                    }
                                    CollabCommand::FlushSnapshot => session.flush_snapshot(),
                                    #[cfg(feature = "voice")]
                                    CollabCommand::StartVoice => {
                                        if voice.is_some() {
                                            continue;
                                        }
                                        match crate::voice::VoiceCall::connect(
                                            &config.video_ws_url(),
                                            config.session_cookie.as_deref(),
                                        )
                                        .await
                                        {
                                            Ok((call, channels)) => {
                                                let crate::voice::VoiceChannels {
                                                    mut events_rx,
                                                    pcm_in_rx,
                                                    pcm_out_tx,
                                                } = channels;
                                                match crate::audio::SpeakerStream::start(pcm_in_rx) {
                                                    Ok(s) => _voice_speaker = Some(s),
                                                    Err(e) => {
                                                        let _ = event_tx.send(
                                                            CollabEvent::VoiceStopped(format!(
                                                                "динамик: {e}"
                                                            )),
                                                        );
                                                    }
                                                }
                                                match crate::audio::MicStream::start(pcm_out_tx) {
                                                    Ok(m) => voice_mic = Some(m),
                                                    Err(e) => {
                                                        let _ = event_tx.send(
                                                            CollabEvent::VoiceStopped(format!(
                                                                "микрофон: {e}"
                                                            )),
                                                        );
                                                    }
                                                }
                                                // Пересылка событий звонка в общий канал.
                                                let forward_tx = event_tx.clone();
                                                tokio::spawn(async move {
                                                    while let Some(ev) = events_rx.recv().await {
                                                        use crate::voice::VoiceEvent as V;
                                                        let mapped = match ev {
                                                            V::SignalingConnected => {
                                                                CollabEvent::VoiceStarted
                                                            }
                                                            V::PeerJoined(id) => {
                                                                CollabEvent::VoicePeerJoined(id)
                                                            }
                                                            V::PeerLeft => CollabEvent::VoicePeerLeft,
                                                            V::RtcConnected => {
                                                                CollabEvent::VoiceRtcConnected
                                                            }
                                                            V::RtcDisconnected(why) => {
                                                                CollabEvent::VoiceStopped(why)
                                                            }
                                                        };
                                                        if forward_tx.send(mapped).is_err() {
                                                            break;
                                                        }
                                                    }
                                                });
                                                // call продолжает жить в переменной voice
                                                voice = Some(call);
                                                let _ = event_tx.send(CollabEvent::VoiceStarted);
                                            }
                                            Err(e) => {
                                                let _ = event_tx.send(CollabEvent::VoiceStopped(
                                                    format!("сигналинг: {e}"),
                                                ));
                                            }
                                        }
                                    }
                                    #[cfg(feature = "voice")]
                                    CollabCommand::StopVoice => {
                                        if let Some(call) = voice.take() {
                                            call.hangup().await;
                                        }
                                        voice_mic = None;
                                        _voice_speaker = None;
                                        let _ = event_tx.send(CollabEvent::VoiceStopped(
                                            "звонок завершён".into(),
                                        ));
                                    }
                                    #[cfg(feature = "voice")]
                                    CollabCommand::SetVoiceMuted(muted) => {
                                        if let Some(mic) = &voice_mic {
                                            mic.set_muted(muted);
                                        }
                                    }
                                    CollabCommand::Close => {
                                        #[cfg(feature = "voice")]
                                        if let Some(call) = voice.take() {
                                            call.hangup().await;
                                        }
                                        session.close().await;
                                        break;
                                    }
                                }
                            }
                        }
                    }
                });
            })
            .expect("spawn collab thread");
        Self {
            cmd_tx,
            event_rx,
            thread: Some(thread),
        }
    }
}

impl Drop for CollabHandle {
    fn drop(&mut self) {
        let _ = self.cmd_tx.send(CollabCommand::Close);
        if let Some(thread) = self.thread.take() {
            // Не ждём вечно: поток завершится сам после Close,
            // а если сеть повисла — detached-поток умрёт с рантаймом.
            let _ = thread.thread().id();
        }
    }
}
