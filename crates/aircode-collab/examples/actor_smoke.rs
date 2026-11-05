//! E2E-смоук актора CollabHandle против живого релея uchi-on-line.
//!
//! Запуск (сервер уже поднят, есть комната и сессия Django):
//!   cargo run -p aircode-collab --example actor_smoke -- \
//!     ws://127.0.0.1:8765 <room> <sessionid> <name>
//!
//! Сценарий: поднять актор, дождаться Started/Synced, отправить LocalChange
//! с пометкой и напечатать все события за ~8 секунд. Параллельно к комнате
//! можно подключить второго клиента (JS/Rust) — его правки должны прийти
//! как TextChanged.

use std::time::Duration;

use aircode_collab::{CollabCommand, CollabConfig, CollabEvent, CollabHandle};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        eprintln!("usage: actor_smoke <ws-base> <room> <sessionid> <name>");
        std::process::exit(2);
    }
    let config = CollabConfig {
        server: args[1].clone(),
        room: args[2].clone(),
        session_cookie: (args[3] != "-").then(|| args[3].clone()),
        user_name: args[4].clone(),
    };
    let mut handle = CollabHandle::spawn(config);
    let deadline = std::time::Instant::now() + Duration::from_secs(8);
    let mut sent = false;
    while std::time::Instant::now() < deadline {
        while let Ok(event) = handle.event_rx.try_recv() {
            match &event {
                CollabEvent::Synced(text) => {
                    println!("[event] Synced: {:?}", truncate(text));
                    if !sent {
                        let mut buffer = text.clone();
                        buffer.push_str("\n// actor-smoke was here\n");
                        handle
                            .cmd_tx
                            .send(CollabCommand::LocalChange(buffer))
                            .expect("cmd channel");
                        println!("[ok] LocalChange sent");
                        sent = true;
                    }
                }
                CollabEvent::TextChanged(text) => {
                    println!("[event] TextChanged: {:?}", truncate(text));
                }
                CollabEvent::PeersChanged(peers) => {
                    let summary: Vec<String> = peers
                        .iter()
                        .map(|p| {
                            format!("{}:{}", p.client_id, p.state_json.as_deref().unwrap_or("-"))
                        })
                        .collect();
                    println!("[event] PeersChanged: {summary:?}");
                }
                other => println!("[event] {other:?}"),
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    println!("[done] sent_marker={sent}");
}

fn truncate(text: &str) -> String {
    if text.chars().count() > 120 {
        let cut: String = text.chars().take(120).collect();
        format!("{cut}…")
    } else {
        text.to_string()
    }
}
