//! E2E голосового звонка AIRcode↔AIRcode через автономный релей.
//!
//! Запуск (релей уже слушает, например `aircode-relay --bind 127.0.0.1:9415`):
//!   cargo run -p aircode-collab --features voice --example voice_pair_smoke -- \
//!     ws://127.0.0.1:9415 <room>
//!
//! Сценарий: два VoiceCall в одной комнате. Оффер создаёт сторона с
//! меньшим connection_id (как в UI). A шлёт тон 440 Гц, B считает кадры.
//! Успех: RtcConnected у обоих + кадры дошли.

#![cfg(feature = "voice")]

use std::time::Duration;

use aircode_collab::voice::{VoiceCall, VoiceEvent};

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        eprintln!("usage: voice_pair_smoke <ws-base> <room>");
        std::process::exit(2);
    }
    let url = format!("{}/ws/video/{}/", args[1].trim_end_matches('/'), args[2]);

    let (a, mut a_ch) = VoiceCall::connect(&url, None).await?;
    println!("[A] signaling connected");
    let (b, mut b_ch) = VoiceCall::connect(&url, None).await?;
    println!("[B] signaling connected");

    let local = tokio::task::LocalSet::new();

    // A: генерируем тон 440 Гц по 20 мс кадрам и логируем события.
    let a_pcm_tx = a_ch.pcm_out_tx.clone();
    let a_task = local.spawn_local(async move {
        let mut phase = 0.0f32;
        let mut connected = false;
        loop {
            tokio::select! {
                Some(ev) = a_ch.events_rx.recv() => {
                    println!("[A] {ev:?}");
                    if matches!(ev, VoiceEvent::RtcConnected) {
                        connected = true;
                    }
                }
                _ = tokio::time::sleep(Duration::from_millis(20)), if connected => {
                    let frame: Vec<i16> = (0..160)
                        .map(|_| {
                            phase += 440.0 * 2.0 * std::f32::consts::PI / 8000.0;
                            (phase.sin() * 12000.0) as i16
                        })
                        .collect();
                    if a_pcm_tx.send(frame).is_err() {
                        break;
                    }
                }
                else => break,
            }
        }
    });

    // B: считаем входящие кадры и события.
    let b_done = local.spawn_local(async move {
        let mut frames = 0usize;
        let mut samples = 0usize;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
        loop {
            tokio::select! {
                Some(ev) = b_ch.events_rx.recv() => println!("[B] {ev:?}"),
                Some(frame) = b_ch.pcm_in_rx.recv() => {
                    frames += 1;
                    samples += frame.len();
                }
                _ = tokio::time::sleep_until(deadline) => break,
                else => break,
            }
        }
        (frames, samples)
    });

    // Ждём B (8 с отсчёта), полля LocalSet; потом гасим таск A.
    let (frames, samples) = local.run_until(b_done).await?;
    drop(a_task);
    a.hangup().await;
    b.hangup().await;
    println!("[B] received {frames} frames / {samples} samples");
    if frames >= 50 {
        println!("VOICE-PAIR:PASS");
        Ok(())
    } else {
        anyhow::bail!("VOICE-PAIR:FAIL — кадров слишком мало");
    }
}
