use std::{collections::HashMap, error::Error, fmt, time::Duration};

use futures_util::StreamExt;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{ApiKey, ChatMessage, ProviderConfig, ToolCall, sse::SseDecoder};

const MAX_STREAM_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq)]
pub enum StreamEvent {
    ReasoningDelta(String),
    ContentDelta(String),
    ToolCall(ToolCall),
    ToolResult(String),
    Usage(serde_json::Value),
    Error(String),
    Done,
}

#[derive(Debug)]
pub enum AiError {
    Credentials(String),
    Settings(String),
    InvalidEndpoint(String),
    Transport(String),
    HttpStatus(u16, String),
    Protocol(String),
    Cancelled,
}

impl fmt::Display for AiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Credentials(message) => write!(formatter, "Credential error: {message}"),
            Self::Settings(message) => write!(formatter, "Settings error: {message}"),
            Self::InvalidEndpoint(message) => write!(formatter, "Invalid API endpoint: {message}"),
            Self::Transport(message) => write!(formatter, "Network error: {message}"),
            Self::HttpStatus(status, message) => write!(formatter, "API error {status}: {message}"),
            Self::Protocol(message) => write!(formatter, "Invalid API response: {message}"),
            Self::Cancelled => write!(formatter, "Request cancelled"),
        }
    }
}

impl Error for AiError {}

pub struct AiClient {
    client: reqwest::Client,
}

impl AiClient {
    pub fn new() -> Result<Self, AiError> {
        let client = reqwest::Client::builder()
            .user_agent(concat!("AIRcode/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(30))
            // Applies between received chunks, so long streams are fine but a dead
            // connection no longer hangs forever.
            .read_timeout(Duration::from_secs(120))
            .build()
            .map_err(|error| AiError::Transport(error.to_string()))?;
        Ok(Self { client })
    }

    pub async fn stream_chat(
        &self,
        config: &ProviderConfig,
        api_key: &ApiKey,
        messages: &[ChatMessage],
        cancel: CancellationToken,
        events: mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<(), AiError> {
        self.stream_chat_with_tools(config, api_key, messages, None, cancel, events)
            .await
    }

    pub async fn stream_chat_with_tools(
        &self,
        config: &ProviderConfig,
        api_key: &ApiKey,
        messages: &[ChatMessage],
        tools: Option<serde_json::Value>,
        cancel: CancellationToken,
        events: mpsc::UnboundedSender<StreamEvent>,
    ) -> Result<(), AiError> {
        if config.model.trim().is_empty() {
            return Err(AiError::InvalidEndpoint(
                "Enter a model ID before sending a message".into(),
            ));
        }
        if config.base_url.trim().is_empty() {
            return Err(AiError::InvalidEndpoint("Enter a base URL".into()));
        }
        let endpoint = reqwest::Url::parse(&config.endpoint())
            .map_err(|error| AiError::InvalidEndpoint(error.to_string()))?;
        if !matches!(endpoint.scheme(), "http" | "https") {
            return Err(AiError::InvalidEndpoint(
                "Only HTTP and HTTPS are supported".into(),
            ));
        }
        let body = config.request_body_with_tools(messages, tools);
        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(AiError::Cancelled),
            result = self.client
                .post(endpoint)
                .bearer_auth(api_key.expose())
                .json(&body)
                .send() => result.map_err(|error| AiError::Transport(error.to_string()))?,
        };

        let status = response.status();
        if !status.is_success() {
            let message = tokio::select! {
                _ = cancel.cancelled() => return Err(AiError::Cancelled),
                text = response.text() => text
                    .unwrap_or_else(|_| "No error details returned".into()),
            };
            let message = message.replace(api_key.expose(), "[REDACTED]");
            let message = match status.as_u16() {
                400 if config.kind == crate::ProviderKind::KimiCode => format!(
                    "{message}. Check the model ID, context limit, and keep thinking enabled."
                ),
                401 => {
                    format!("{message}. Check that the API key belongs to the selected service.")
                }
                403 if config.kind == crate::ProviderKind::KimiCode => format!(
                    "{message}. Check your Kimi membership model access and context allowance."
                ),
                429 => format!("{message}. Rate limit or quota reached; retry later."),
                _ => message,
            };
            return Err(AiError::HttpStatus(status.as_u16(), message));
        }

        let mut decoder = SseDecoder::new();
        let mut tool_calls = HashMap::new();
        let mut stream = response.bytes_stream();
        let mut received = 0usize;
        let mut finished = false;
        loop {
            let chunk = tokio::select! {
                _ = cancel.cancelled() => return Err(AiError::Cancelled),
                chunk = stream.next() => chunk,
            };
            let (batch, closed) = match chunk {
                Some(chunk) => {
                    let chunk = chunk.map_err(|error| AiError::Transport(error.to_string()))?;
                    received += chunk.len();
                    if received > MAX_STREAM_BYTES {
                        return Err(AiError::Protocol("Response exceeded 64 MiB".into()));
                    }
                    (decoder.push(&chunk).map_err(AiError::Protocol)?, false)
                }
                None => (decoder.finish().map_err(AiError::Protocol)?, true),
            };
            for data in batch {
                if forward_event(&data, &events, &mut finished, &mut tool_calls)? {
                    emit_tool_calls(&mut tool_calls, &events);
                    let _ = events.send(StreamEvent::Done);
                    return Ok(());
                }
            }
            if closed {
                break;
            }
        }
        if !finished {
            return Err(AiError::Protocol(
                "The stream ended before the response was complete".into(),
            ));
        }
        emit_tool_calls(&mut tool_calls, &events);
        emit_tool_calls(&mut tool_calls, &events);
        let _ = events.send(StreamEvent::Done);
        Ok(())
    }
}

/// Handles one SSE `data:` payload. Returns `true` on the `[DONE]` sentinel.
fn forward_event(
    data: &str,
    events: &mpsc::UnboundedSender<StreamEvent>,
    finished: &mut bool,
    tool_calls: &mut HashMap<usize, ToolCall>,
) -> Result<bool, AiError> {
    if data.trim() == "[DONE]" {
        *finished = true;
        return Ok(true);
    }
    let chunk: ChatCompletionChunk =
        serde_json::from_str(data).map_err(|error| AiError::Protocol(error.to_string()))?;
    if let Some(error) = chunk.error {
        let message = error
            .get("message")
            .and_then(|value| value.as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| error.to_string());
        return Err(AiError::Protocol(message));
    }
    if let Some(usage) = chunk.usage {
        let _ = events.send(StreamEvent::Usage(usage));
    }
    for choice in chunk.choices {
        if choice.finish_reason.is_some() {
            *finished = true;
        }
        let Some(delta) = choice.delta else {
            continue;
        };
        merge_tool_calls(tool_calls, delta.tool_calls);
        if let Some(reasoning) = delta.reasoning_content
            && !reasoning.is_empty()
        {
            let _ = events.send(StreamEvent::ReasoningDelta(reasoning));
        }
        if let Some(content) = delta.content
            && !content.is_empty()
        {
            let _ = events.send(StreamEvent::ContentDelta(content));
        }
    }
    Ok(false)
}

#[derive(Deserialize)]
struct ChatCompletionChunk {
    #[serde(default)]
    choices: Vec<Choice>,
    usage: Option<serde_json::Value>,
    error: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct Choice {
    /// Some gateways send a final choice without `delta` (or with `"delta": null`).
    delta: Option<Delta>,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct Delta {
    content: Option<String>,
    reasoning_content: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ToolCallDelta>,
}

#[derive(Deserialize)]
struct ToolCallDelta {
    index: usize,
    id: Option<String>,
    function: ToolFunctionDelta,
}

#[derive(Deserialize)]
struct ToolFunctionDelta {
    name: Option<String>,
    arguments: String,
}

fn merge_tool_calls(map: &mut HashMap<usize, ToolCall>, deltas: Vec<ToolCallDelta>) {
    for delta in deltas {
        let call = map.entry(delta.index).or_insert_with(|| ToolCall {
            id: String::new(),
            name: String::new(),
            arguments: String::new(),
        });
        if let Some(id) = delta.id {
            call.id = id;
        }
        if let Some(name) = delta.function.name {
            call.name = name;
        }
        call.arguments.push_str(&delta.function.arguments);
    }
}

fn emit_tool_calls(
    map: &mut HashMap<usize, ToolCall>,
    events: &mpsc::UnboundedSender<StreamEvent>,
) {
    let mut calls = map.drain().collect::<Vec<_>>();
    calls.sort_by_key(|(index, _)| *index);
    for (_, call) in calls {
        if !call.name.is_empty() {
            let _ = events.send(StreamEvent::ToolCall(call));
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
    };

    use super::*;
    use crate::{ProviderKind, Region};

    async fn server(
        status: &'static str,
        content_type: &'static str,
        body: &'static str,
        fragments: usize,
    ) -> (String, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            loop {
                let count = socket.read(&mut buffer).await.unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..count]);
                if let Some(headers_end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..headers_end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .and_then(|value| value.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if request.len() >= headers_end + 4 + length {
                        break;
                    }
                }
            }
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            for fragment in response.as_bytes().chunks(fragments.max(1)) {
                socket.write_all(fragment).await.unwrap();
                socket.flush().await.unwrap();
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            String::from_utf8(request).unwrap()
        });
        (format!("http://{address}/v1"), task)
    }

    fn key() -> ApiKey {
        ApiKey::new("sk-test-secret".into()).unwrap()
    }

    #[tokio::test]
    async fn streams_reasoning_content_and_usage() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"Think \"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"Hello\"}}],\"usage\":{\"total_tokens\":8}}\n\n",
            "data: [DONE]\n\n"
        );
        let (base_url, request) = server("200 OK", "text/event-stream", body, 7).await;
        let mut config = ProviderConfig::default().with_region(Region::China);
        config.base_url = base_url;
        let (sender, mut receiver) = mpsc::unbounded_channel();

        AiClient::new()
            .unwrap()
            .stream_chat(
                &config,
                &key(),
                &[ChatMessage::user("Hi")],
                CancellationToken::new(),
                sender,
            )
            .await
            .unwrap();

        let mut received = Vec::new();
        while let Some(event) = receiver.recv().await {
            received.push(event);
        }
        assert_eq!(
            received,
            vec![
                StreamEvent::ReasoningDelta("Think ".into()),
                StreamEvent::Usage(serde_json::json!({"total_tokens": 8})),
                StreamEvent::ContentDelta("Hello".into()),
                StreamEvent::Done,
            ]
        );
        let request = request.await.unwrap();
        assert!(request.contains("AIRcode/"));
        assert!(request.contains("\"model\":\"k3.1-pro\""));
        assert!(request.contains("\"thinking\":{\"type\":\"enabled\"}"));
    }

    #[tokio::test]
    async fn assembles_tool_calls_from_fragments() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-1\",\"function\":{\"name\":\"write_file\",\"arguments\":\"{\\\"path\\\":\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"src/main.rs\\\",\\\"content\\\":\\\"fn main() {}\\\"}\"}}]}}]}\n\n",
            "data: [DONE]\n\n"
        );
        let (base_url, _) = server("200 OK", "text/event-stream", body, 3).await;
        let config = ProviderConfig {
            base_url,
            ..ProviderConfig::default()
        };
        let (sender, mut receiver) = mpsc::unbounded_channel();

        AiClient::new()
            .unwrap()
            .stream_chat_with_tools(
                &config,
                &key(),
                &[],
                Some(serde_json::json!([])),
                CancellationToken::new(),
                sender,
            )
            .await
            .unwrap();

        let mut events = Vec::new();
        while let Some(event) = receiver.recv().await {
            events.push(event);
        }
        assert_eq!(
            events,
            vec![
                StreamEvent::ToolCall(ToolCall {
                    id: "call-1".into(),
                    name: "write_file".into(),
                    arguments: "{\"path\":\"src/main.rs\",\"content\":\"fn main() {}\"}".into(),
                }),
                StreamEvent::Done,
            ]
        );
    }

    #[tokio::test]
    async fn http_errors_are_readable_and_redact_the_key() {
        let (base_url, _) = server(
            "401 Unauthorized",
            "application/json",
            "{\"error\":{\"message\":\"bad key sk-test-secret\"}}",
            1024,
        )
        .await;
        let mut config = ProviderConfig::new(ProviderKind::DeepSeek);
        config.base_url = base_url;
        let (sender, _) = mpsc::unbounded_channel();

        let error = AiClient::new()
            .unwrap()
            .stream_chat(&config, &key(), &[], CancellationToken::new(), sender)
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("401"));
        assert!(error.contains("Check that the API key"));
        assert!(!error.contains("sk-test-secret"));
    }

    #[tokio::test]
    async fn cancellation_stops_the_stream() {
        let (base_url, _) = server(
            "200 OK",
            "text/event-stream",
            "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"}}]}\n\n",
            1,
        )
        .await;
        let config = ProviderConfig {
            base_url,
            ..ProviderConfig::default()
        };
        let cancel = CancellationToken::new();
        cancel.cancel();
        let (sender, _) = mpsc::unbounded_channel();

        let result = AiClient::new()
            .unwrap()
            .stream_chat(&config, &key(), &[], cancel, sender)
            .await;

        assert!(matches!(result, Err(AiError::Cancelled)));
    }

    #[tokio::test]
    async fn cancellation_interrupts_an_active_stream() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"first\"}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"content\":\"second\"}}]}\n\n",
            "data: [DONE]\n\n"
        );
        let (base_url, _) = server("200 OK", "text/event-stream", body, 1).await;
        let config = ProviderConfig {
            base_url,
            ..ProviderConfig::default()
        };
        let cancel = CancellationToken::new();
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let client = std::sync::Arc::new(AiClient::new().unwrap());
        let task = {
            let cancel = cancel.clone();
            tokio::spawn(async move {
                client
                    .stream_chat(&config, &key(), &[], cancel, sender)
                    .await
            })
        };

        let first = tokio::time::timeout(Duration::from_secs(10), receiver.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first, StreamEvent::ContentDelta("first".into()));
        cancel.cancel();

        assert!(matches!(task.await.unwrap(), Err(AiError::Cancelled)));
    }
}
