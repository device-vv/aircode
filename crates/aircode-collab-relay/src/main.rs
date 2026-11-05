//! Отдельный бинарник релея: `aircode-relay [--bind 0.0.0.0:9415]`.
//!
//! AIRcode умеет поднимать тот же релей встроенно (host-режим); этот
//! бинарник — для запуска на выделенной машине/сервере, если удобнее так.

use std::net::SocketAddr;

fn main() {
    let mut bind: SocketAddr = "0.0.0.0:9415".parse().expect("default bind");
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--bind" if i + 1 < args.len() => {
                bind = args[i + 1].parse().unwrap_or_else(|_| {
                    eprintln!("некорректный адрес: {}", args[i + 1]);
                    std::process::exit(2);
                });
                i += 2;
            }
            "-h" | "--help" => {
                println!("usage: aircode-relay [--bind 0.0.0.0:9415]");
                return;
            }
            other => {
                eprintln!("неизвестный аргумент: {other}");
                std::process::exit(2);
            }
        }
    }

    let relay = match aircode_collab_relay::RelayHandle::spawn(bind) {
        Ok(relay) => relay,
        Err(error) => {
            eprintln!("не удалось поднять релей на {bind}: {error}");
            std::process::exit(1);
        }
    };
    let addr = relay.addr();
    println!("AIRcode relay слушает на {addr}");
    println!("Подключение из AIRcode:");
    println!("  код:  ws://{addr}/ws/code/<комната>");
    println!("  голос: ws://{addr}/ws/video/<комната>");
    println!("Ctrl+C — остановить.");

    // Спим вечно; Ctrl+C убьёт процесс, RelayHandle дропнется вместе с ним.
    let _relay = relay;
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}
