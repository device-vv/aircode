//! Голосовой звонок 1-на-1 через сигналинг `/ws/video/<room_id>/`.
//!
//! Медиа — G.711 PCMU (µ-law, 8 кГц, кадры по 20 мс = 160 сэмплов): кодек
//! обязателен для браузеров, поэтому десктоп совместим с веб-клиентом
//! uchi-on-line без внешних библиотек. Захват/воспроизведение звука в crate
//! не входят: приложение подаёт PCM через [`VoiceCall::pcm_out_tx`] и
//! забирает из [`VoiceCall::pcm_in_rx`] (в AIRcode это будет cpal).
//!
//! Ограничение v0.1: один собеседник (peer). Mesh-комнаты до 4 человек —
//! по одному `VoiceCall`-peer'у на участника, как в `static/js/video.js`.

use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::http::Request;
use tokio_tungstenite::tungstenite::Message;

use webrtc::api::media_engine::MediaEngine;
use webrtc::api::APIBuilder;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::ice_transport::ice_server::RTCIceServer;
use webrtc::media::Sample;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::rtp_transceiver::rtp_codec::{RTCRtpCodecCapability, RTCRtpCodecParameters, RTPCodecType};
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use webrtc::track::track_local::TrackLocal;

use crate::protocol::signal;

/// G.711 µ-law кодек (классический g711.c).
pub mod g711 {
    const SEG_END: [i32; 8] = [0xFF, 0x1FF, 0x3FF, 0x7FF, 0xFFF, 0x1FFF, 0x3FFF, 0x7FFF];
    const BIAS: i32 = 0x84;
    const CLIP: i32 = 32635;

    /// i16 PCM -> µ-law байт.
    pub fn linear_to_ulaw(mut pcm: i32) -> u8 {
        let mask = if pcm < 0 {
            pcm = BIAS - pcm;
            0x7F
        } else {
            pcm += BIAS;
            0xFF
        };
        if pcm > CLIP {
            pcm = CLIP;
        }
        let seg = SEG_END.iter().position(|&e| pcm <= e).unwrap_or(8);
        if seg >= 8 {
            return (0x7F ^ mask) as u8;
        }
        let uval = ((seg as i32) << 4) | ((pcm >> (seg + 3)) & 0xF);
        (uval ^ mask) as u8
    }

    /// µ-law байт -> i16 PCM.
    pub fn ulaw_to_linear(u: u8) -> i16 {
        let u = !(u as i32) & 0xFF;
        let mut t = ((u & 0x0F) << 3) + BIAS;
        t <<= (u & 0x70) >> 4;
        if (u & 0x80) != 0 {
            (BIAS - t) as i16
        } else {
            (t - BIAS) as i16
        }
    }

    /// Кодировать PCM-кадр в µ-law.
    pub fn encode(pcm: &[i16]) -> Vec<u8> {
        pcm.iter().map(|&s| linear_to_ulaw(s as i32)).collect()
    }

    /// Декодировать µ-law кадр в PCM.
    pub fn decode(ulaw: &[u8]) -> Vec<i16> {
        ulaw.iter().map(|&b| ulaw_to_linear(b)).collect()
    }
}

/// События звонка для UI.
#[derive(Debug, Clone)]
pub enum VoiceEvent {
    /// Сигнальный WebSocket подключён.
    SignalingConnected,
    /// Вошёл собеседник (`connection_id`).
    PeerJoined(String),
    /// Собеседник вышел.
    PeerLeft,
    /// WebRTC-соединение установлено, звук пошёл.
    RtcConnected,
    /// WebRTC-соединение закрыто/потеряно.
    RtcDisconnected(String),
}

/// Каналы звонка для аудио-стека и UI (отделяются от ядра при подключении).
pub struct VoiceChannels {
    /// События для UI.
    pub events_rx: mpsc::UnboundedReceiver<VoiceEvent>,
    /// Входящий звук собеседника: PCM i16, 8 кГц mono (кадры по 160 сэмплов).
    pub pcm_in_rx: mpsc::UnboundedReceiver<Vec<i16>>,
    /// Исходящий звук (микрофон): PCM i16, 8 кГц mono, кратно 160 сэмплам.
    pub pcm_out_tx: mpsc::UnboundedSender<Vec<i16>>,
}

/// Детерминированный выбор инициатора: оффер создаёт сторона с меньшим
/// `connection_id`. Вызывается и на PEER_JOINED, и на READY — правило
/// симметрично, поэтому оффер уходит ровно один раз.
async fn maybe_make_offer(
    pc: &webrtc::peer_connection::RTCPeerConnection,
    sig_tx: &mpsc::UnboundedSender<Message>,
    me: &Option<String>,
    peer: &Option<String>,
) {
    let (Some(me), Some(peer)) = (me, peer) else {
        return;
    };
    if me >= peer {
        return;
    }
    if pc.remote_description().await.is_some() {
        return; // оффер уже пришёл с той стороны
    }
    let Ok(offer) = pc.create_offer(None).await else {
        return;
    };
    if pc.set_local_description(offer).await.is_err() {
        return;
    }
    let Some(local) = pc.local_description().await else {
        return;
    };
    let _ = sig_tx.send(Message::Text(
        serde_json::json!({
            "type": signal::OFFER,
            "kind": signal::KIND_CAM,
            "to": peer,
            "sdp": {"type": "offer", "sdp": local.sdp}
        })
        .to_string()
        .into(),
    ));
}

/// Голосовой звонок в комнате (сигналинг + медиа-задачи).
pub struct VoiceCall {
    sig_tx: mpsc::UnboundedSender<Message>,
    tasks: Vec<JoinHandle<()>>,
}

impl VoiceCall {
    /// Подключиться к сигналингу и ждать/принимать звонок.
    ///
    /// Против браузера (веб-клиент) оффер создаёт он; AIRcode↔AIRcode
    /// оффер детерминированно создаёт сторона с меньшим `connection_id`.
    /// Возвращает ядро звонка и каналы событий/звука.
    pub async fn connect(
        ws_url: &str,
        session_cookie: Option<&str>,
    ) -> anyhow::Result<(Self, VoiceChannels)> {
        let sep = if ws_url.contains('?') { "&" } else { "?" };
        let url_sid = format!("{ws_url}{sep}sid=aircode-{:x}", std::process::id());
        let mut builder = Request::builder()
            .uri(&url_sid)
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

        let (sig_tx, mut sig_rx) = mpsc::unbounded_channel::<Message>();
        let mut tasks = Vec::new();
        tasks.push(tokio::spawn(async move {
            while let Some(m) = sig_rx.recv().await {
                if ws_tx.send(m).await.is_err() {
                    break;
                }
            }
        }));

        let (events_tx, events_rx) = mpsc::unbounded_channel::<VoiceEvent>();
        let (pcm_in_tx, pcm_in_rx) = mpsc::unbounded_channel::<Vec<i16>>();
        let (pcm_out_tx, mut pcm_out_rx) = mpsc::unbounded_channel::<Vec<i16>>();
        let _ = events_tx.send(VoiceEvent::SignalingConnected);

        // --- WebRTC: только PCMU, чтобы браузер слал µ-law ---
        let mut media_engine = MediaEngine::default();
        media_engine.register_codec(
            RTCRtpCodecParameters {
                capability: RTCRtpCodecCapability {
                    mime_type: "audio/PCMU".to_owned(),
                    clock_rate: 8000,
                    channels: 1,
                    ..Default::default()
                },
                payload_type: 0,
                ..Default::default()
            },
            RTPCodecType::Audio,
        )?;
        let api = APIBuilder::new().with_media_engine(media_engine).build();
        let config = RTCConfiguration {
            ice_servers: vec![RTCIceServer {
                urls: vec!["stun:stun.l.google.com:19302".to_owned()],
                ..Default::default()
            }],
            ..Default::default()
        };
        let pc = Arc::new(api.new_peer_connection(config).await?);

        let track = Arc::new(TrackLocalStaticSample::new(
            RTCRtpCodecCapability {
                mime_type: "audio/PCMU".to_owned(),
                clock_rate: 8000,
                channels: 1,
                ..Default::default()
            },
            "audio".to_owned(),
            "aircode-voice".to_owned(),
        ));
        pc.add_track(Arc::clone(&track) as Arc<dyn TrackLocal + Send + Sync>)
            .await?;

        let peer_id: Arc<tokio::sync::Mutex<Option<String>>> =
            Arc::new(tokio::sync::Mutex::new(None));
        // Наш собственный connection_id (из welcome) — нужен для
        // детерминированного выбора, кто создаёт оффер: меньший id звонит.
        let self_id: Arc<tokio::sync::Mutex<Option<String>>> =
            Arc::new(tokio::sync::Mutex::new(None));

        // ICE trickle -> сигналинг
        pc.on_ice_candidate(Box::new({
            let sig_tx = sig_tx.clone();
            let peer_id = peer_id.clone();
            move |cand| {
                let sig_tx = sig_tx.clone();
                let peer_id = peer_id.clone();
                Box::pin(async move {
                    let Some(cand) = cand else { return };
                    let Ok(init) = cand.to_json() else { return };
                    let mut msg = serde_json::json!({
                        "type": signal::ICE,
                        "kind": signal::KIND_CAM,
                        "candidate": {
                            "candidate": init.candidate,
                            "sdpMid": init.sdp_mid,
                            "sdpMLineIndex": init.sdp_mline_index,
                        }
                    });
                    if let Some(to) = peer_id.lock().await.clone() {
                        msg["to"] = serde_json::Value::String(to);
                    }
                    let _ = sig_tx.send(Message::Text(msg.to_string().into()));
                })
            }
        }));

        pc.on_peer_connection_state_change(Box::new({
            let events_tx = events_tx.clone();
            move |state| {
                let events_tx = events_tx.clone();
                Box::pin(async move {
                    use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState::*;
                    match state {
                        Connected => {
                            let _ = events_tx.send(VoiceEvent::RtcConnected);
                        }
                        Failed | Disconnected | Closed => {
                            let _ = events_tx
                                .send(VoiceEvent::RtcDisconnected(format!("{state}")));
                        }
                        _ => {}
                    }
                })
            }
        }));

        // Входящий звук -> PCM канал
        pc.on_track(Box::new(move |remote, _receiver, _transceiver| {
            let pcm_in_tx = pcm_in_tx.clone();
            Box::pin(async move {
                tokio::spawn(async move {
                    while let Ok((rtp, _)) = remote.read_rtp().await {
                        let _ = pcm_in_tx.send(g711::decode(&rtp.payload));
                    }
                });
            })
        }));

        // Исходящий звук: PCM -> µ-law -> RTP (20 мс кадры)
        tasks.push(tokio::spawn(async move {
            let mut pending: Vec<i16> = Vec::new();
            while let Some(frame) = pcm_out_rx.recv().await {
                pending.extend_from_slice(&frame);
                while pending.len() >= 160 {
                    let chunk: Vec<i16> = pending.drain(..160).collect();
                    if track
                        .write_sample(&Sample {
                            data: g711::encode(&chunk).into(),
                            duration: Duration::from_millis(20),
                            ..Default::default()
                        })
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            }
        }));

        // --- сигнальный цикл ---
        tasks.push(tokio::spawn({
            let sig_tx = sig_tx.clone();
            let events_tx = events_tx.clone();
            let peer_id = peer_id.clone();
            let self_id = self_id.clone();
            let pc = pc.clone();
            async move {
                while let Some(msg) = ws_rx.next().await {
                    let Ok(Message::Text(text)) = msg else { continue };
                    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
                        continue;
                    };
                    let send = |v: serde_json::Value| {
                        let _ = sig_tx.send(Message::Text(v.to_string().into()));
                    };
                    match v["type"].as_str().unwrap_or("") {
                        signal::WELCOME => {
                            let id = v["connection_id"].as_str().unwrap_or("").to_string();
                            if !id.is_empty() {
                                *self_id.lock().await = Some(id);
                            }
                        }
                        signal::PEER_JOINED => {
                            let id = v["connection_id"].as_str().unwrap_or("").to_string();
                            if id.is_empty() {
                                continue;
                            }
                            *peer_id.lock().await = Some(id.clone());
                            let _ = events_tx.send(VoiceEvent::PeerJoined(id.clone()));
                            send(serde_json::json!({"type": signal::READY, "to": id}));
                            maybe_make_offer(
                                &pc,
                                &sig_tx,
                                &self_id.lock().await.clone(),
                                &peer_id.lock().await.clone(),
                            )
                            .await;
                        }
                        signal::READY => {
                            if let Some(id) = v["connection_id"].as_str().map(|s| s.to_string()) {
                                let mut p = peer_id.lock().await;
                                if p.is_none() {
                                    *p = Some(id);
                                }
                            }
                            send(serde_json::json!({
                                "type": signal::MEDIA_STATE, "camera": false, "microphone": true
                            }));
                            // AIRcode↔AIRcode: браузерной стороны-инициатора
                            // нет, поэтому оффер создаёт детерминированно
                            // тот, у кого connection_id меньше.
                            maybe_make_offer(
                                &pc,
                                &sig_tx,
                                &self_id.lock().await.clone(),
                                &peer_id.lock().await.clone(),
                            )
                            .await;
                        }
                        signal::ANSWER => {
                            if v["kind"].as_str() != Some(signal::KIND_CAM) {
                                continue;
                            }
                            let sdp = v["sdp"]["sdp"].as_str().unwrap_or("").to_string();
                            if let Ok(answer) = RTCSessionDescription::answer(sdp) {
                                let _ = pc.set_remote_description(answer).await;
                            }
                        }
                        signal::OFFER => {
                            if v["kind"].as_str() != Some(signal::KIND_CAM) {
                                continue;
                            }
                            let from = v["connection_id"].as_str().unwrap_or("").to_string();
                            *peer_id.lock().await = Some(from.clone());
                            let sdp = v["sdp"]["sdp"].as_str().unwrap_or("").to_string();
                            let Ok(offer) = RTCSessionDescription::offer(sdp) else { continue };
                            if pc.set_remote_description(offer).await.is_err() {
                                continue;
                            }
                            let Ok(answer) = pc.create_answer(None).await else { continue };
                            if pc.set_local_description(answer).await.is_err() {
                                continue;
                            }
                            let Some(local) = pc.local_description().await else { continue };
                            send(serde_json::json!({
                                "type": signal::ANSWER,
                                "kind": signal::KIND_CAM,
                                "to": from,
                                "sdp": {"type": "answer", "sdp": local.sdp}
                            }));
                        }
                        signal::ICE => {
                            let c = &v["candidate"];
                            let candidate = c["candidate"].as_str().unwrap_or("").to_string();
                            if candidate.is_empty() {
                                continue;
                            }
                            let _ = pc
                                .add_ice_candidate(RTCIceCandidateInit {
                                    candidate,
                                    sdp_mid: c["sdpMid"].as_str().map(|s| s.to_string()),
                                    sdp_mline_index: c["sdpMLineIndex"]
                                        .as_u64()
                                        .map(|i| i as u16),
                                    ..Default::default()
                                })
                                .await;
                        }
                        signal::PEER_LEFT | signal::HANGUP => {
                            *peer_id.lock().await = None;
                            let _ = events_tx.send(VoiceEvent::PeerLeft);
                        }
                        _ => {}
                    }
                }
            }
        }));

        Ok((
            Self { sig_tx, tasks },
            VoiceChannels {
                events_rx,
                pcm_in_rx,
                pcm_out_tx,
            },
        ))
    }

    /// Повесить трубку: hangup всем + остановка задач.
    pub async fn hangup(self) {
        let _ = self.sig_tx.send(Message::Text(
            serde_json::json!({"type": signal::HANGUP}).to_string().into(),
        ));
        tokio::time::sleep(Duration::from_millis(150)).await;
        for t in &self.tasks {
            t.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::g711::*;

    #[test]
    fn ulaw_roundtrip() {
        for s in [-30000i16, -1000, -100, 0, 100, 1000, 30000] {
            let back = ulaw_to_linear(linear_to_ulaw(s as i32));
            assert!((back as i32 - s as i32).abs() < 900, "{s} -> {back}");
        }
    }
}
