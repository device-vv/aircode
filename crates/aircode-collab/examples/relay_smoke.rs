//! Смоук-тест crate aircode-collab против живого релея uchi-on-line.
//!
//! Запуск (сервер уже поднят, есть комната и сессия Django):
//!   cargo run -p aircode-collab --example relay_smoke -- \
//!     ws://127.0.0.1:8765/ws/code/<room>/ <sessionid> Smoke-Rust ["текст"]

use aircode_collab::CollabSession;
use aircode_collab::session::CollabEvent;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: relay_smoke <ws-url> <sessionid> <name> [text]");
        std::process::exit(2);
    }
    let (url, session, name) = (args[1].clone(), args[2].clone(), args[3].clone());
    let insert = args.get(4).cloned();

    let cookie = (session != "-").then_some(session.as_str());
    let mut sess = CollabSession::connect(&url, cookie, &name).await?;
    println!("[ok] connected");

    if let Some(t) = insert {
        sess.insert(u32::MAX, &t);
        sess.flush_snapshot();
        println!("[ok] inserted + snapshot flushed");
    }

    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(6);
    loop {
        match tokio::time::timeout_at(deadline, sess.next_event()).await {
            Ok(Some(ev)) => match ev {
                CollabEvent::RemoteTextChanged => {
                    println!("[event] RemoteTextChanged: {:?}", sess.text())
                }
                other => println!("[event] {other:?}"),
            },
            Ok(None) => break,
            Err(_) => break,
        }
    }

    println!("[done] final text = {:?}", sess.text());
    sess.close().await;
    Ok(())
}
