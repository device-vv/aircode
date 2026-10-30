use std::{
    collections::HashMap,
    error::Error,
    fmt,
    time::{Duration, Instant},
};

use futures_util::StreamExt;
use serde::Deserialize;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::{ApiKey, ChatMessage, ProviderConfig, ToolCall, sse::SseDecoder};

const MAX_STREAM_BYTES: usize = 64 * 1024 * 1024;
const MAX_TRANSIENT_RETRIES: usize = 2;
const MAX_WRITE_PROGRESS_BYTES: usize = 16 * 1024;
const WRITE_PROGRESS_INTERVAL: Duration = Duration::from_millis(120);
const STREAM_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const STREAM_HEADER_TIMEOUT: Duration = Duration::from_secs(60);
#[cfg(not(test))]
const RETRY_BASE_DELAY: Duration = Duration::from_millis(300);
#[cfg(test)]
const RETRY_BASE_DELAY: Duration = Duration::from_millis(10);

#[derive(Clone, Debug, PartialEq)]
pub enum StreamEvent {
    ReasoningDelta(String),
    ContentDelta(String),
    ToolCalls(Vec<ToolCall>),
    WriteFileProgress {
        call_id: String,
        path: String,
        content: String,
        content_truncated: bool,
    },
    ToolResult {
        call_id: String,
        result: String,
    },
    Usage(serde_json::Value),
    Retrying {
        attempt: u8,
        max_attempts: u8,
        delay_ms: u64,
    },
    Error(String),
    Done,
}

#[derive(Debug)]
pub enum AiError {
    Credentials(String),
    InvalidEndpoint(String),
    Transport(String),
    HttpStatus(u16, String),
    Protocol(String),
    Incomplete(String),
    Cancelled,
}

impl fmt::Display for AiError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Credentials(message) => write!(formatter, "Credential error: {message}"),
            Self::InvalidEndpoint(message) => write!(formatter, "Invalid API endpoint: {message}"),
            Self::Transport(message) => write!(formatter, "Network error: {message}"),
            Self::HttpStatus(status, message) => write!(formatter, "API error {status}: {message}"),
            Self::Protocol(message) => write!(formatter, "Invalid API response: {message}"),
            Self::Incomplete(message) => write!(formatter, "Incomplete AI response: {message}"),
            Self::Cancelled => write!(formatter, "Request cancelled"),
        }
    }
}

impl Error for AiError {}

#[derive(Clone, Copy)]
struct StreamTimeouts {
    header: Duration,
    idle: Duration,
    retry_base: Duration,
}

impl Default for StreamTimeouts {
    fn default() -> Self {
        Self {
            header: STREAM_HEADER_TIMEOUT,
            idle: STREAM_IDLE_TIMEOUT,
            retry_base: RETRY_BASE_DELAY,
        }
    }
}

pub struct AiClient {
    client: reqwest::Client,
    timeouts: StreamTimeouts,
}

impl AiClient {
    pub fn new() -> Result<Self, AiError> {
        let client = reqwest::Client::builder()
            .user_agent(concat!("AIRcode/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(30))
            // Bound idle reads while allowing long, actively streaming responses.
            .read_timeout(Duration::from_secs(120))
            .build()
            .map_err(|error| AiError::Transport(error.to_string()))?;
        Ok(Self {
            client,
            timeouts: StreamTimeouts::default(),
        })
    }

    #[cfg(test)]
    fn with_test_timeouts(mut self, header: Duration, idle: Duration) -> Self {
        self.timeouts.header = header;
        self.timeouts.idle = idle;
        self.timeouts.retry_base = Duration::from_millis(10);
        self
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
        let mut attempt = 0usize;
        loop {
            let mut exposed = false;
            let result = self
                .stream_once(
                    &endpoint,
                    api_key,
                    &body,
                    config,
                    &cancel,
                    &events,
                    &mut exposed,
                )
                .await;
            match result {
                Ok(()) => return Ok(()),
                Err(error)
                    if !exposed && attempt < MAX_TRANSIENT_RETRIES && is_transient(&error) =>
                {
                    attempt += 1;
                    let delay = self.timeouts.retry_base.saturating_mul(attempt as u32);
                    let _ = events.send(StreamEvent::Retrying {
                        attempt: attempt as u8,
                        max_attempts: MAX_TRANSIENT_RETRIES as u8,
                        delay_ms: delay.as_millis().min(u64::MAX as u128) as u64,
                    });
                    tokio::select! {
                        biased;
                        _ = cancel.cancelled() => return Err(AiError::Cancelled),
                        _ = tokio::time::sleep(delay) => {}
                    }
                }
                Err(error) => return Err(error),
            }
        }
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "One attempt shares the request, cancellation, and retry exposure state"
    )]
    async fn stream_once(
        &self,
        endpoint: &reqwest::Url,
        api_key: &ApiKey,
        body: &serde_json::Value,
        config: &ProviderConfig,
        cancel: &CancellationToken,
        events: &mpsc::UnboundedSender<StreamEvent>,
        exposed: &mut bool,
    ) -> Result<(), AiError> {
        let response = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(AiError::Cancelled),
            result = tokio::time::timeout(
                self.timeouts.header,
                self.client
                    .post(endpoint.clone())
                    .bearer_auth(api_key.expose())
                    .json(body)
                    .send(),
            ) => {
                match result {
                    Ok(result) => {
                        result.map_err(|error| AiError::Transport(error.to_string()))?
                    }
                    Err(_) => {
                        return Err(AiError::Transport(format!(
                            "The AI response stalled for {} seconds while waiting for headers",
                            self.timeouts.header.as_secs_f32()
                        )));
                    }
                }
            },
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
        let mut last_progress = Instant::now();
        let mut progress = WriteProgressState::default();
        loop {
            let deadline = tokio::time::Instant::from_std(last_progress + self.timeouts.idle);
            if tokio::time::Instant::now() >= deadline {
                return Err(AiError::Transport(format!(
                    "The AI stream stalled for {} seconds",
                    self.timeouts.idle.as_secs_f32()
                )));
            }
            let chunk = tokio::select! {
                biased;
                _ = cancel.cancelled() => return Err(AiError::Cancelled),
                chunk = tokio::time::timeout_at(deadline, stream.next()) => {
                    chunk.map_err(|_| AiError::Transport(format!(
                        "The AI stream stalled for {} seconds",
                        self.timeouts.idle.as_secs_f32()
                    )))?
                },
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
                if forward_event(
                    &data,
                    events,
                    &mut finished,
                    &mut tool_calls,
                    api_key.expose(),
                    &mut last_progress,
                    exposed,
                    &mut progress,
                )? {
                    require_response_output(&tool_calls, &progress)?;
                    emit_tool_calls(&mut tool_calls, events, exposed)?;
                    *exposed = true;
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
        require_response_output(&tool_calls, &progress)?;
        emit_tool_calls(&mut tool_calls, events, exposed)?;
        *exposed = true;
        let _ = events.send(StreamEvent::Done);
        Ok(())
    }
}

fn is_transient(error: &AiError) -> bool {
    matches!(
        error,
        AiError::Transport(_) | AiError::HttpStatus(408 | 429 | 502 | 503 | 504, _)
    )
}

/// Forward one SSE payload. Returns true for the terminal `[DONE]` sentinel.
#[expect(
    clippy::too_many_arguments,
    reason = "The decoder updates protocol and progress state together for each payload"
)]
fn forward_event(
    data: &str,
    events: &mpsc::UnboundedSender<StreamEvent>,
    finished: &mut bool,
    tool_calls: &mut HashMap<usize, ToolCall>,
    api_key: &str,
    last_progress: &mut Instant,
    exposed: &mut bool,
    progress: &mut WriteProgressState,
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
        return Err(AiError::Protocol(message.replace(api_key, "[REDACTED]")));
    }
    if let Some(usage) = chunk.usage {
        *exposed = true;
        let _ = events.send(StreamEvent::Usage(usage));
    }
    for choice in chunk.choices {
        let Some(delta) = choice.delta else {
            check_finish_reason(choice.finish_reason, finished)?;
            continue;
        };
        if !delta.tool_calls.is_empty() {
            *exposed = true;
            merge_tool_calls(tool_calls, delta.tool_calls);
            *last_progress = Instant::now();
            emit_write_progress(tool_calls, events, progress);
        }
        if let Some(reasoning) = delta.reasoning_content
            && !reasoning.is_empty()
        {
            *exposed = true;
            *last_progress = Instant::now();
            let _ = events.send(StreamEvent::ReasoningDelta(reasoning));
        }
        if let Some(content) = delta.content
            && !content.is_empty()
        {
            progress.has_content |= !content.trim().is_empty();
            *exposed = true;
            *last_progress = Instant::now();
            let _ = events.send(StreamEvent::ContentDelta(content));
        }
        check_finish_reason(choice.finish_reason, finished)?;
    }
    Ok(false)
}

fn check_finish_reason(reason: Option<String>, finished: &mut bool) -> Result<(), AiError> {
    let Some(reason) = reason else {
        return Ok(());
    };
    match reason.as_str() {
        "stop" | "tool_calls" | "function_call" => {
            *finished = true;
            Ok(())
        }
        "length" => Err(AiError::Incomplete(
            "the provider stopped at its token limit. The text above is partial; ask the model to continue or retry with a smaller request."
                .into(),
        )),
        "content_filter" => Err(AiError::Incomplete(
            "the provider blocked the response with a content filter. The text above may be partial.".into(),
        )),
        "error" => Err(AiError::Incomplete(
            "the provider reported an error finish reason. The text above may be partial.".into(),
        )),
        other => Err(AiError::Protocol(format!(
            "unsupported finish_reason: {other}"
        ))),
    }
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
    #[serde(default)]
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

#[derive(Default)]
struct WriteProgressState {
    has_content: bool,
    last_emit: Option<Instant>,
    last_signature: Option<(String, String, usize, bool)>,
}

fn require_response_output(
    calls: &HashMap<usize, ToolCall>,
    progress: &WriteProgressState,
) -> Result<(), AiError> {
    if !progress.has_content && calls.is_empty() {
        return Err(AiError::Incomplete(
            "the model returned no answer or tool action; the task was not completed".into(),
        ));
    }
    Ok(())
}

fn emit_write_progress(
    tool_calls: &HashMap<usize, ToolCall>,
    events: &mpsc::UnboundedSender<StreamEvent>,
    state: &mut WriteProgressState,
) {
    let Some((_, call)) = tool_calls
        .iter()
        .filter(|(_, call)| call.name == "write_file")
        .min_by_key(|(index, _)| **index)
    else {
        return;
    };
    let now = Instant::now();
    if let Some(last_emit) = state.last_emit
        && now.duration_since(last_emit) < WRITE_PROGRESS_INTERVAL
    {
        return;
    }
    let Some((path, content, content_truncated)) = extract_write_file_progress(&call.arguments)
    else {
        return;
    };
    let signature = (
        call.id.clone(),
        path.clone(),
        content.len(),
        content_truncated,
    );
    state.last_emit = Some(now);
    if state.last_signature.as_ref() == Some(&signature) {
        return;
    }
    state.last_signature = Some(signature);
    let _ = events.send(StreamEvent::WriteFileProgress {
        call_id: call.id.clone(),
        path,
        content,
        content_truncated,
    });
}

fn extract_write_file_progress(arguments: &str) -> Option<(String, String, bool)> {
    let bytes = arguments.as_bytes();
    let mut index = skip_json_ws(arguments, 0);
    if bytes.get(index) != Some(&b'{') {
        return None;
    }
    index += 1;
    let mut path = None;
    let mut content = None;
    let mut content_truncated = false;
    loop {
        index = skip_json_ws(arguments, index);
        match bytes.get(index) {
            None | Some(&b'}') => break,
            Some(&b',') => {
                index += 1;
                continue;
            }
            Some(&b'"') => {}
            _ => return None,
        }
        let (key, key_closed, _, next) = parse_json_string_prefix(arguments, index + 1, 256)?;
        if !key_closed {
            return None;
        }
        index = skip_json_ws(arguments, next);
        if bytes.get(index) != Some(&b':') {
            return None;
        }
        index = skip_json_ws(arguments, index + 1);
        match key.as_str() {
            "path" => {
                if bytes.get(index) != Some(&b'"') {
                    return None;
                }
                let (value, closed, truncated, next) =
                    parse_json_string_prefix(arguments, index + 1, MAX_WRITE_PROGRESS_BYTES)?;
                if !closed || truncated {
                    return None;
                }
                path = Some(value);
                index = next;
            }
            "content" => {
                if bytes.get(index) != Some(&b'"') {
                    return None;
                }
                let (value, closed, truncated, next) =
                    parse_json_string_prefix(arguments, index + 1, MAX_WRITE_PROGRESS_BYTES)?;
                content = Some(value);
                content_truncated = truncated;
                index = next;
                if !closed {
                    break;
                }
            }
            _ => index = skip_json_value(arguments, index)?,
        }
        index = skip_json_ws(arguments, index);
        match bytes.get(index) {
            None | Some(&b'}') => break,
            Some(&b',') => index += 1,
            _ => return None,
        }
    }
    Some((path?, content.unwrap_or_default(), content_truncated))
}

fn skip_json_ws(input: &str, mut index: usize) -> usize {
    let bytes = input.as_bytes();
    while let Some(byte) = bytes.get(index) {
        if !matches!(*byte, b' ' | b'\n' | b'\r' | b'\t') {
            break;
        }
        index += 1;
    }
    index
}

fn skip_json_value(input: &str, index: usize) -> Option<usize> {
    match *input.as_bytes().get(index)? {
        b'"' => {
            let (_, closed, _, next) = parse_json_string_prefix(input, index + 1, 0)?;
            closed.then_some(next)
        }
        b'{' | b'[' => skip_json_container(input, index),
        _ => {
            let bytes = input.as_bytes();
            let mut end = index;
            while let Some(byte) = bytes.get(end)
                && !matches!(*byte, b',' | b'}' | b']')
            {
                end += 1;
            }
            Some(end)
        }
    }
}

fn skip_json_container(input: &str, start: usize) -> Option<usize> {
    let bytes = input.as_bytes();
    let mut depth = 0usize;
    let mut index = start;
    loop {
        match *bytes.get(index)? {
            b'"' => {
                let (_, closed, _, next) = parse_json_string_prefix(input, index + 1, 0)?;
                if !closed {
                    return None;
                }
                index = next;
            }
            b'{' | b'[' => {
                depth += 1;
                index += 1;
            }
            b'}' | b']' => {
                depth = depth.checked_sub(1)?;
                index += 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => index += 1,
        }
    }
}

fn parse_json_string_prefix(
    input: &str,
    mut index: usize,
    max_bytes: usize,
) -> Option<(String, bool, bool, usize)> {
    let mut value = String::new();
    let mut truncated = false;
    loop {
        let Some(ch) = input[index..].chars().next() else {
            return Some((value, false, truncated, index));
        };
        if ch == '"' {
            return Some((value, true, truncated, index + 1));
        }
        if ch != '\\' {
            index += ch.len_utf8();
            if !push_bounded_json_char(&mut value, ch, max_bytes) {
                truncated = true;
            }
            continue;
        }
        let Some(&escape) = input.as_bytes().get(index + 1) else {
            return Some((value, false, truncated, index));
        };
        let (decoded, consumed) = match escape {
            b'"' => ('"', 2),
            b'\\' => ('\\', 2),
            b'/' => ('/', 2),
            b'b' => ('\u{8}', 2),
            b'f' => ('\u{c}', 2),
            b'n' => ('\n', 2),
            b'r' => ('\r', 2),
            b't' => ('\t', 2),
            b'u' => {
                if input.len() < index + 6 {
                    return Some((value, false, truncated, index));
                }
                let unit = hex4(input, index + 2)?;
                let (code, consumed) = if (0xD800..=0xDBFF).contains(&unit) {
                    if input.len() < index + 12 {
                        return Some((value, false, truncated, index));
                    }
                    if input.get(index + 6..index + 8) != Some("\\u") {
                        return None;
                    }
                    let low = hex4(input, index + 8)?;
                    if !(0xDC00..=0xDFFF).contains(&low) {
                        return None;
                    }
                    (
                        0x10000 + (((unit as u32) - 0xD800) << 10) + ((low as u32) - 0xDC00),
                        12,
                    )
                } else {
                    (unit as u32, 6)
                };
                (char::from_u32(code)?, consumed)
            }
            _ => return None,
        };
        index += consumed;
        if !push_bounded_json_char(&mut value, decoded, max_bytes) {
            truncated = true;
        }
    }
}

fn push_bounded_json_char(value: &mut String, ch: char, max_bytes: usize) -> bool {
    if value.len().saturating_add(ch.len_utf8()) > max_bytes {
        return false;
    }
    value.push(ch);
    true
}

fn hex4(input: &str, start: usize) -> Option<u16> {
    let bytes = input.as_bytes().get(start..start.checked_add(4)?)?;
    let mut value = 0u16;
    for byte in bytes {
        value = (value << 4)
            | match *byte {
                b'0'..=b'9' => (*byte - b'0') as u16,
                b'a'..=b'f' => (*byte - b'a' + 10) as u16,
                b'A'..=b'F' => (*byte - b'A' + 10) as u16,
                _ => return None,
            };
    }
    Some(value)
}

fn emit_tool_calls(
    map: &mut HashMap<usize, ToolCall>,
    events: &mpsc::UnboundedSender<StreamEvent>,
    exposed: &mut bool,
) -> Result<(), AiError> {
    let mut calls = map.drain().collect::<Vec<_>>();
    calls.sort_by_key(|(index, _)| *index);
    let calls = calls.into_iter().map(|(_, call)| call).collect::<Vec<_>>();
    if calls
        .iter()
        .any(|call| call.id.trim().is_empty() || call.name.trim().is_empty())
    {
        return Err(AiError::Protocol(
            "A tool call is missing its id or function name".into(),
        ));
    }
    if !calls.is_empty() {
        *exposed = true;
        let _ = events.send(StreamEvent::ToolCalls(calls));
    }
    Ok(())
}

#[cfg(test)]
fn assert_tool_call_protocol_errors() {
    let mut calls = HashMap::from([(
        0,
        ToolCall {
            id: String::new(),
            name: "read_file".into(),
            arguments: "{}".into(),
        },
    )]);
    let (sender, _) = mpsc::unbounded_channel();
    let mut exposed = false;
    assert!(emit_tool_calls(&mut calls, &sender, &mut exposed).is_err());
}

#[cfg(test)]
mod protocol_tests {
    #[test]
    fn incomplete_tool_calls_are_rejected() {
        super::assert_tool_call_protocol_errors();
    }
}

#[cfg(test)]
mod tests {
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
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

    async fn read_request(socket: &mut TcpStream) -> String {
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
        String::from_utf8(request).unwrap()
    }

    async fn retry_server() -> (String, tokio::task::JoinHandle<usize>) {
        const OK_BODY: &str = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"recovered\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n"
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let mut requests = 0;
            while requests < 2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                requests += 1;
                let _ = read_request(&mut socket).await;
                let (status, body) = if requests == 1 {
                    (
                        "429 Too Many Requests",
                        "{\"error\":{\"message\":\"slow down\"}}",
                    )
                } else {
                    ("200 OK", OK_BODY)
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
            requests
        });
        (format!("http://{address}/v1"), task)
    }

    fn key() -> ApiKey {
        ApiKey::new("sk-test-secret".into()).unwrap()
    }

    #[test]
    fn write_progress_extracts_fragmented_escapes_and_unicode() {
        let arguments = concat!(
            r#"{"path":"src/\ud83e\udd80.rs","content":"first line\n"#,
            r#"\"quoted\"\t\u041f\u0440\u0438\u0432\u0435\u0442 \ud83e\udd80"}"#
        );
        let path = "src/🦀.rs";
        let content = "first line\n\"quoted\"\tПривет 🦀";
        let mut fragment = String::new();
        let mut saw_path = false;
        let mut saw_content = false;

        for ch in arguments.chars() {
            fragment.push(ch);
            if let Some((fragment_path, fragment_content, truncated)) =
                extract_write_file_progress(&fragment)
            {
                assert_eq!(fragment_path, path);
                assert!(content.starts_with(fragment_content.as_str()));
                assert!(!truncated);
                saw_path = true;
                saw_content |= !fragment_content.is_empty();
            }
        }

        assert!(saw_path);
        assert!(saw_content);
        assert_eq!(
            extract_write_file_progress(arguments).unwrap(),
            (path.into(), content.into(), false)
        );
    }

    #[test]
    fn write_progress_accepts_any_key_order_and_incomplete_content() {
        let arguments = concat!(
            r#"{"content":"fn main() {\n    // Привет 🦀\n}","#,
            r#""path":"src/main.rs"}"#
        );
        assert_eq!(
            extract_write_file_progress(arguments).unwrap(),
            (
                "src/main.rs".into(),
                "fn main() {\n    // Привет 🦀\n}".into(),
                false,
            )
        );

        let incomplete = r#"{"content":"unterminated \u041"#;
        assert!(extract_write_file_progress(incomplete).is_none());
        let with_path = r#"{"path":"src/lib.rs","content":"unterminated \u041"#;
        assert_eq!(
            extract_write_file_progress(with_path).unwrap(),
            ("src/lib.rs".into(), "unterminated ".into(), false)
        );
    }

    #[test]
    fn write_progress_snapshot_is_bounded_on_a_character_boundary() {
        let content = "🦀".repeat(MAX_WRITE_PROGRESS_BYTES / 4 + 100);
        let arguments = serde_json::json!({
            "path": "src/large.rs",
            "content": content,
        })
        .to_string();
        let (path, snapshot, truncated) = extract_write_file_progress(&arguments).unwrap();

        assert_eq!(path, "src/large.rs");
        assert!(truncated);
        assert!(snapshot.len() <= MAX_WRITE_PROGRESS_BYTES);
        assert!(snapshot.is_char_boundary(snapshot.len()));
        assert!(content.starts_with(snapshot.as_str()));
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
                StreamEvent::WriteFileProgress {
                    call_id: "call-1".into(),
                    path: "src/main.rs".into(),
                    content: "fn main() {}".into(),
                    content_truncated: false,
                },
                StreamEvent::ToolCalls(vec![ToolCall {
                    id: "call-1".into(),
                    name: "write_file".into(),
                    arguments: "{\"path\":\"src/main.rs\",\"content\":\"fn main() {}\"}".into(),
                }]),
                StreamEvent::Done,
            ]
        );
    }

    #[tokio::test]
    async fn batches_multiple_tool_calls_in_index_order_with_reasoning() {
        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"Keep this\",\"tool_calls\":[{\"index\":1,\"id\":\"call-2\",\"function\":{\"name\":\"search_text\",\"arguments\":\"{}\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-1\",\"function\":{\"name\":\"read_file\"}}]}}]}\n\n",
            "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"path\\\":\\\"src/lib.rs\\\"}\"}}]}}]}\n\n",
            "data: [DONE]\n\n"
        );
        let (base_url, _) = server("200 OK", "text/event-stream", body, 2).await;
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
                StreamEvent::ReasoningDelta("Keep this".into()),
                StreamEvent::ToolCalls(vec![
                    ToolCall {
                        id: "call-1".into(),
                        name: "read_file".into(),
                        arguments: "{\"path\":\"src/lib.rs\"}".into(),
                    },
                    ToolCall {
                        id: "call-2".into(),
                        name: "search_text".into(),
                        arguments: "{}".into(),
                    },
                ]),
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
    async fn eof_flushes_a_complete_final_event() {
        let body =
            "data: {\"choices\":[{\"delta\":{\"content\":\"tail\"},\"finish_reason\":\"stop\"}]}\n";
        let (base_url, _) = server("200 OK", "text/event-stream", body, 5).await;
        let config = ProviderConfig {
            base_url,
            ..ProviderConfig::default()
        };
        let (sender, mut receiver) = mpsc::unbounded_channel();

        AiClient::new()
            .unwrap()
            .stream_chat(&config, &key(), &[], CancellationToken::new(), sender)
            .await
            .unwrap();

        let events = receiver.recv().await.unwrap();
        assert_eq!(events, StreamEvent::ContentDelta("tail".into()));
        assert_eq!(receiver.recv().await.unwrap(), StreamEvent::Done);
    }

    #[tokio::test]
    async fn truncated_stream_is_not_reported_as_done() {
        let body = "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n";
        let (base_url, _) = server("200 OK", "text/event-stream", body, 1024).await;
        let config = ProviderConfig {
            base_url,
            ..ProviderConfig::default()
        };
        let (sender, _) = mpsc::unbounded_channel();

        let error = AiClient::new()
            .unwrap()
            .stream_chat(&config, &key(), &[], CancellationToken::new(), sender)
            .await
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("before the response was complete")
        );
    }

    #[tokio::test]
    async fn in_stream_errors_are_surfaced_and_redacted() {
        let body = concat!(
            "data: {\"error\":{\"message\":\"bad key sk-test-secret\"}}\n\n",
            "data: [DONE]\n\n"
        );
        let (base_url, _) = server("200 OK", "text/event-stream", body, 1024).await;
        let config = ProviderConfig {
            base_url,
            ..ProviderConfig::default()
        };
        let (sender, _) = mpsc::unbounded_channel();

        let error = AiClient::new()
            .unwrap()
            .stream_chat(&config, &key(), &[], CancellationToken::new(), sender)
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("[REDACTED]"));
        assert!(!error.contains("sk-test-secret"));
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

    #[tokio::test]
    async fn length_and_content_filter_are_incomplete_not_done() {
        for (reason, expected) in [
            ("length", "token limit"),
            ("content_filter", "content filter"),
        ] {
            let body = Box::leak(
                format!(
                    "data: {{\"choices\":[{{\"delta\":{{\"content\":\"partial\"}},\"finish_reason\":\"{reason}\"}}]}}\n\ndata: [DONE]\n\n"
                )
                .into_boxed_str(),
            );
            let (base_url, _) = server("200 OK", "text/event-stream", body, 1024).await;
            let config = ProviderConfig {
                base_url,
                ..ProviderConfig::default()
            };
            let (sender, mut receiver) = mpsc::unbounded_channel();
            let error = AiClient::new()
                .unwrap()
                .stream_chat(&config, &key(), &[], CancellationToken::new(), sender)
                .await
                .unwrap_err();
            assert!(matches!(&error, AiError::Incomplete(_)));
            assert!(error.to_string().contains(expected), "{error}");
            let mut events = Vec::new();
            while let Some(event) = receiver.recv().await {
                events.push(event);
            }
            assert_eq!(
                events,
                vec![StreamEvent::ContentDelta("partial".into())],
                "an incomplete response must not emit Done"
            );
        }
    }

    #[tokio::test]
    async fn empty_and_reasoning_only_responses_never_complete_a_task() {
        for body in [
            "data: [DONE]\n\n",
            "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"thinking\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n",
        ] {
            let (base_url, request) = server("200 OK", "text/event-stream", body, 1024).await;
            let config = ProviderConfig {
                base_url,
                ..ProviderConfig::default()
            };
            let (sender, mut receiver) = mpsc::unbounded_channel();
            let error = AiClient::new()
                .unwrap()
                .stream_chat(&config, &key(), &[], CancellationToken::new(), sender)
                .await
                .unwrap_err();
            assert!(matches!(error, AiError::Incomplete(_)));
            while let Some(event) = receiver.recv().await {
                assert!(!matches!(
                    event,
                    StreamEvent::Done | StreamEvent::ToolCalls(_)
                ));
            }
            request.await.unwrap();
        }
    }

    #[tokio::test]
    async fn response_header_stall_has_a_bounded_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                tokio::spawn(async move {
                    let _ = read_request(&mut socket).await;
                    tokio::time::sleep(Duration::from_secs(5)).await;
                });
            }
        });
        let config = ProviderConfig {
            base_url: format!("http://{address}/v1"),
            ..ProviderConfig::default()
        };
        let (sender, _) = mpsc::unbounded_channel();
        let error = tokio::time::timeout(
            Duration::from_secs(4),
            AiClient::new()
                .unwrap()
                .with_test_timeouts(Duration::from_millis(250), STREAM_IDLE_TIMEOUT)
                .stream_chat(&config, &key(), &[], CancellationToken::new(), sender),
        )
        .await
        .expect("stalled stream exceeded the test deadline")
        .unwrap_err();
        assert!(error.to_string().contains("stalled"), "{error}");
        assert!(error.to_string().contains("headers"), "{error}");
        server.abort();
    }

    #[tokio::test]
    async fn heartbeat_only_stream_does_not_extend_the_semantic_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let _ = read_request(&mut socket).await;
            socket
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            for _ in 0..40 {
                if socket.write_all(b": keepalive\n\n").await.is_err() {
                    break;
                }
                let _ = socket.flush().await;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        let config = ProviderConfig {
            base_url: format!("http://{address}/v1"),
            ..ProviderConfig::default()
        };
        let (sender, _) = mpsc::unbounded_channel();
        let error = tokio::time::timeout(
            Duration::from_secs(4),
            AiClient::new()
                .unwrap()
                .with_test_timeouts(Duration::from_millis(250), Duration::from_millis(250))
                .stream_chat(&config, &key(), &[], CancellationToken::new(), sender),
        )
        .await
        .expect("heartbeat-only stream exceeded the test deadline")
        .unwrap_err();
        assert!(error.to_string().contains("stalled"), "{error}");
        server.abort();
    }

    #[tokio::test]
    async fn cancellation_interrupts_the_response_header_wait() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let _ = read_request(&mut socket).await;
            tokio::time::sleep(Duration::from_secs(2)).await;
        });
        let config = ProviderConfig {
            base_url: format!("http://{address}/v1"),
            ..ProviderConfig::default()
        };
        let cancel = CancellationToken::new();
        let (sender, _) = mpsc::unbounded_channel();
        let client = std::sync::Arc::new(AiClient::new().unwrap());
        let task = {
            let cancel = cancel.clone();
            tokio::spawn(async move {
                client
                    .stream_chat(&config, &key(), &[], cancel, sender)
                    .await
            })
        };
        tokio::time::sleep(Duration::from_millis(40)).await;
        cancel.cancel();

        let result = tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .expect("header cancellation exceeded the test deadline")
            .unwrap();
        assert!(matches!(result, Err(AiError::Cancelled)));
        server.abort();
    }

    #[tokio::test]
    async fn transient_rate_limit_retries_before_exposing_output() {
        let (base_url, requests) = retry_server().await;
        let config = ProviderConfig {
            base_url,
            ..ProviderConfig::default()
        };
        let (sender, mut receiver) = mpsc::unbounded_channel();
        AiClient::new()
            .unwrap()
            .stream_chat(&config, &key(), &[], CancellationToken::new(), sender)
            .await
            .unwrap();
        let mut events = Vec::new();
        while let Some(event) = receiver.recv().await {
            events.push(event);
        }
        assert!(matches!(
            events[0],
            StreamEvent::Retrying { attempt: 1, .. }
        ));
        assert!(events.contains(&StreamEvent::ContentDelta("recovered".into())));
        assert_eq!(events.last(), Some(&StreamEvent::Done));
        assert_eq!(requests.await.unwrap(), 2);
    }

    #[tokio::test]
    async fn partial_output_prevents_a_retry_and_duplicate_actions() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let server_attempts = attempts.clone();
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                server_attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let _ = read_request(&mut socket).await;
                let body = "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call-1\",\"function\":{\"name\":\"write_file\",\"arguments\":\"{\\\"path\\\":\\\"src/main.rs\\\"}\"}}]}}]}\n\n";
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let config = ProviderConfig {
            base_url: format!("http://{address}/v1"),
            ..ProviderConfig::default()
        };
        let (sender, _) = mpsc::unbounded_channel();
        let error = AiClient::new()
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
            .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("before the response was complete")
        );
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert_eq!(
            attempts.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "a retry after exposed tool output could duplicate an action"
        );
        server.abort();
    }
}
