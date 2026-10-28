// Reproductions of bugs found during review. Each test asserts the CORRECT
// behaviour, so a failing test == confirmed bug.
use std::time::Duration;

use aircode_ai::{AiClient, ApiKey, ChatMessage, ProviderConfig, ProviderKind, StreamEvent};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::mpsc,
};
use tokio_util::sync::CancellationToken;

async fn serve(body: &'static str) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0u8; 65536];
        let mut req = Vec::new();
        loop {
            let n = socket.read(&mut buf).await.unwrap();
            req.extend_from_slice(&buf[..n]);
            if let Some(h) = req.windows(4).position(|w| w == b"\r\n\r\n") {
                let head = String::from_utf8_lossy(&req[..h]).to_ascii_lowercase();
                let len = head
                    .lines()
                    .find_map(|l| {
                        l.strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if req.len() >= h + 4 + len {
                    break;
                }
            }
        }
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(resp.as_bytes()).await.unwrap();
        socket.flush().await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
    });
    format!("http://{address}/v1")
}

async fn run(body: &'static str) -> (Result<(), String>, Vec<StreamEvent>) {
    let mut config = ProviderConfig::new(ProviderKind::OpenAiCompatible);
    config.base_url = serve(body).await;
    config.model = "m".into();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let result = AiClient::new()
        .unwrap()
        .stream_chat(
            &config,
            &ApiKey::new("k".into()).unwrap(),
            &[ChatMessage::user("hi")],
            CancellationToken::new(),
            tx,
        )
        .await
        .map_err(|e| e.to_string());
    let mut events = Vec::new();
    while let Some(e) = rx.recv().await {
        events.push(e);
    }
    (result, events)
}

/// Final chunk `{"index":0,"finish_reason":"stop"}` without `delta`
/// (sent by several OpenAI-compatible gateways) must not abort the answer.
#[tokio::test]
async fn bug_choice_without_delta_aborts_stream() {
    let (result, events) = run(concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello\"}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n"
    ))
    .await;
    assert!(
        result.is_ok(),
        "stream failed: {result:?}; events: {events:?}"
    );
}

/// `"delta": null` must be tolerated as well.
#[tokio::test]
async fn bug_null_delta_aborts_stream() {
    let (result, _) = run(concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":null,\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n"
    ))
    .await;
    assert!(result.is_ok(), "stream failed: {result:?}");
}

/// An error object sent inside the SSE stream (HTTP 200) must surface as an
/// error, not as a silently "successful" empty answer.
#[tokio::test]
async fn bug_in_stream_error_is_swallowed() {
    let (result, events) = run(concat!(
        "data: {\"error\":{\"message\":\"quota exceeded\",\"code\":429}}\n\n",
        "data: [DONE]\n\n"
    ))
    .await;
    let surfaced = result.is_err() || events.iter().any(|e| matches!(e, StreamEvent::Error(_)));
    assert!(surfaced, "error was swallowed: {result:?} {events:?}");
}

/// The last SSE event without a trailing blank line (connection closed right
/// after it) is silently dropped.
#[tokio::test]
async fn bug_last_event_without_blank_line_is_lost() {
    let (_, events) = run("data: {\"choices\":[{\"delta\":{\"content\":\"tail\"}}]}\n").await;
    assert!(
        events.contains(&StreamEvent::ContentDelta("tail".into())),
        "tail lost: {events:?}"
    );
}

/// A truncated stream (no [DONE], connection closed) is reported as a normal
/// successful completion.
#[tokio::test]
async fn bug_truncated_stream_reported_as_success() {
    let (result, _) =
        run("data: {\"choices\":[{\"delta\":{\"content\":\"half an ans\"}}]}\n\n").await;
    assert!(result.is_err(), "truncated stream treated as complete");
}

/// The key is stored under `label:base_url`, so a cosmetic edit of the URL
/// (trailing slash) "loses" the saved key although the endpoint is identical.
#[test]
fn bug_credential_id_depends_on_url_formatting() {
    let mut a = ProviderConfig::new(ProviderKind::OpenAiCompatible);
    a.base_url = "https://example.com/v1".into();
    let mut b = a.clone();
    b.base_url = "https://example.com/v1/".into();
    assert_eq!(a.endpoint(), b.endpoint());
    assert_eq!(
        a.credential_id(),
        b.credential_id(),
        "same endpoint, different key slot"
    );
}

/// Settings written by a future/older version with a missing provider field
/// make the WHOLE settings file unreadable (ProviderConfig has no serde default).
#[test]
fn bug_missing_provider_field_breaks_settings() {
    let dir = std::env::temp_dir().join(format!("aircode-repro-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("settings.toml");
    std::fs::write(
        &path,
        "include_document = false\n[provider]\nkind = \"kimi_code\"\nregion = \"china\"\nbase_url = \"https://api.kimi.com/coding/v1\"\nmodel = \"k3\"\n",
    )
    .unwrap();
    let loaded = aircode_ai::SettingsStore::at(&path).load();
    std::fs::remove_dir_all(&dir).ok();
    assert!(loaded.is_ok(), "settings rejected: {:?}", loaded.err());
}

/// Mirrors Workspace::select_provider: the provider button cycles through all 5
/// providers via `ProviderConfig::new(kind).with_region(region)`. Non-Kimi
/// presets force Region::Overseas, so one full cycle silently moves a
/// Kimi Code (China) user to the overseas endpoint (and another key slot).
#[test]
fn bug_provider_cycle_resets_kimi_region() {
    let mut provider = ProviderConfig::default(); // Kimi Code, China
    for _ in 0..ProviderKind::ALL.len() {
        let current = ProviderKind::ALL
            .iter()
            .position(|k| *k == provider.kind)
            .unwrap();
        let kind = ProviderKind::ALL[(current + 1) % ProviderKind::ALL.len()];
        let region = provider.region;
        provider = ProviderConfig::new(kind).with_region(region);
    }
    assert_eq!(provider.kind, ProviderKind::KimiCode);
    assert_eq!(
        provider.base_url, "https://api.kimi.com/coding/v1",
        "region flipped"
    );
}
