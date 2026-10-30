use std::{
    collections::{HashSet, VecDeque},
    ops::Range,
};

use aircode_ai::{ChatMessage, MessageRole, ToolCall};

const MAX_TOOL_RESULT_BYTES: usize = 32_000;
const MAX_CONTEXT_MESSAGES: usize = 48;
const MAX_CONTEXT_BYTES: usize = 500_000;
const CONTEXT_NOTICE: &str = "Context truncated: {omitted} older messages were omitted. The latest user goal and system instructions are retained. Project inspection may be incomplete; do not claim the entire project was scanned.";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResponseEnd {
    Final,
    ToolCalls,
    Ignored,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NextTool {
    Execute(ToolCall),
    FollowUp,
    Busy,
}

#[derive(Default)]
struct Response {
    active: bool,
    content: String,
    reasoning: String,
    calls: Vec<ToolCall>,
}

/// Protocol state for one user message, including all of its tool follow-ups.
#[derive(Default)]
pub struct AgentTurn {
    messages: Vec<ChatMessage>,
    response: Response,
    pending: VecDeque<ToolCall>,
    active: Option<ToolCall>,
    seen_tool_calls: HashSet<String>,
}

impl AgentTurn {
    pub fn begin(&mut self, messages: Vec<ChatMessage>) {
        *self = Self {
            messages,
            ..Self::default()
        };
    }

    pub fn begin_response(&mut self) {
        self.response = Response {
            active: true,
            ..Response::default()
        };
    }

    pub fn push_content(&mut self, delta: &str) {
        if self.response.active {
            self.response.content.push_str(delta);
        }
    }

    pub fn push_reasoning(&mut self, delta: &str) {
        if self.response.active {
            self.response.reasoning.push_str(delta);
        }
    }

    pub fn push_tool_calls(&mut self, calls: Vec<ToolCall>) {
        if self.response.active {
            for call in calls {
                // Providers can replay a completed chunk after a reconnect or
                // duplicate an SSE event. Tool calls are side effects, so only
                // accept each call id once for the whole turn.
                if !call.id.is_empty() && self.seen_tool_calls.insert(call.id.clone()) {
                    self.response.calls.push(call);
                }
            }
        }
    }

    pub fn finish_response(&mut self) -> ResponseEnd {
        if !std::mem::replace(&mut self.response.active, false) {
            return ResponseEnd::Ignored;
        }
        if self.response.calls.is_empty() {
            if !self.response.content.is_empty() {
                self.messages.push(ChatMessage {
                    role: MessageRole::Assistant,
                    content: std::mem::take(&mut self.response.content),
                    reasoning_content: Some(std::mem::take(&mut self.response.reasoning)),
                    tool_calls: Vec::new(),
                    tool_call_id: None,
                });
            }
            return ResponseEnd::Final;
        }

        let calls = std::mem::take(&mut self.response.calls);
        self.messages.push(ChatMessage::assistant_tool_calls(
            std::mem::take(&mut self.response.content),
            std::mem::take(&mut self.response.reasoning),
            &calls,
        ));
        self.pending = calls.into();
        ResponseEnd::ToolCalls
    }

    pub fn next_tool(&mut self) -> NextTool {
        if self.active.is_some() {
            return NextTool::Busy;
        }
        let Some(call) = self.pending.pop_front() else {
            return NextTool::FollowUp;
        };
        self.active = Some(call.clone());
        NextTool::Execute(call)
    }

    pub fn complete_tool(&mut self, call_id: &str, result: String) -> Result<(), String> {
        let Some(active) = self.active.as_ref() else {
            return Err("Received a tool result with no active tool call".into());
        };
        if active.id != call_id {
            return Err(format!(
                "Tool result id {call_id} does not match active call {}",
                active.id
            ));
        }
        let result = truncate_tool_result(result);
        self.messages
            .push(ChatMessage::tool_result(call_id, result));
        self.active = None;
        Ok(())
    }

    pub fn cancel(&mut self) {
        self.response.active = false;
        self.pending.clear();
        self.active = None;
    }

    pub fn request_messages(&self) -> Result<Vec<ChatMessage>, String> {
        bounded_context(&self.messages)
    }
}

fn truncate_tool_result(result: String) -> String {
    if result.len() <= MAX_TOOL_RESULT_BYTES {
        return result;
    }
    let mut end = MAX_TOOL_RESULT_BYTES;
    while !result.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n...[tool result truncated: {} of {} bytes omitted; project inspection is incomplete]",
        &result[..end],
        result.len() - end,
        result.len()
    )
}

fn message_bytes(message: &ChatMessage) -> usize {
    message.content.len()
        + message.reasoning_content.as_ref().map_or(0, String::len)
        + message.tool_call_id.as_ref().map_or(0, String::len)
        + message
            .tool_calls
            .iter()
            .map(|call| {
                call.id.len() + call.function.name.len() + call.function.arguments.len() + 32
            })
            .sum::<usize>()
        + 32
}

fn context_units(messages: &[ChatMessage]) -> Vec<Range<usize>> {
    let mut units = Vec::new();
    let mut index = 0;
    while index < messages.len() {
        let mut end = index + 1;
        if !messages[index].tool_calls.is_empty() {
            while end < messages.len() && messages[end].role == MessageRole::Tool {
                end += 1;
            }
        }
        units.push(index..end);
        index = end;
    }
    units
}

fn complete_tool_exchange(unit: &Range<usize>, messages: &[ChatMessage]) -> bool {
    let calls = messages[unit.clone()]
        .iter()
        .flat_map(|message| message.tool_calls.iter())
        .collect::<Vec<_>>();
    if calls.is_empty() {
        return false;
    }
    let results = messages[unit.clone()]
        .iter()
        .filter(|message| message.role == MessageRole::Tool)
        .filter_map(|message| message.tool_call_id.as_deref())
        .collect::<HashSet<_>>();
    calls.iter().all(|call| results.contains(call.id.as_str()))
}

/// Keep system instructions, the latest user goal, and as much recent complete
/// history as fits. Tool calls and their results are never split.
fn bounded_context(messages: &[ChatMessage]) -> Result<Vec<ChatMessage>, String> {
    if messages.is_empty() {
        return Ok(Vec::new());
    }
    let total_bytes = messages.iter().map(message_bytes).sum::<usize>();
    if messages.len() <= MAX_CONTEXT_MESSAGES && total_bytes <= MAX_CONTEXT_BYTES {
        return Ok(messages.to_vec());
    }

    let units = context_units(messages);
    let goal = messages
        .iter()
        .rposition(|message| message.role == MessageRole::User);
    let pinned_systems = messages
        .iter()
        .enumerate()
        .filter(|(_, message)| message.role == MessageRole::System)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let pinned_count = pinned_systems.len() + usize::from(goal.is_some());
    if pinned_count + 1 > MAX_CONTEXT_MESSAGES {
        return Err(
            "System instructions and the latest goal exceed the context message limit".into(),
        );
    }
    let pinned_bytes = pinned_systems
        .iter()
        .chain(goal.iter())
        .map(|index| message_bytes(&messages[*index]))
        .sum::<usize>();
    // The notice is synthetic and its omitted-count placeholder can grow.
    let reserved = CONTEXT_NOTICE.len() + 32 + 24;
    let byte_budget = MAX_CONTEXT_BYTES - reserved;
    if pinned_bytes > byte_budget {
        return Err("System instructions and the latest goal exceed the context byte limit".into());
    }
    let message_budget = MAX_CONTEXT_MESSAGES - pinned_count - 1;
    let mut selected = vec![false; messages.len()];
    for index in pinned_systems.iter().chain(goal.iter()) {
        selected[*index] = true;
    }

    let latest_exchange = units
        .iter()
        .rposition(|unit| complete_tool_exchange(unit, messages));
    if let Some(index) = latest_exchange {
        let unit = &units[index];
        let unit_bytes = messages[unit.clone()]
            .iter()
            .map(message_bytes)
            .sum::<usize>();
        if unit_bytes > byte_budget.saturating_sub(pinned_bytes) || unit.len() > message_budget {
            return Err(format!(
                "The latest complete tool exchange requires {unit_bytes} bytes in {} messages and cannot fit the remaining context budget",
                unit.len()
            ));
        }
        for message in unit.clone() {
            selected[message] = true;
        }
    }

    let mut bytes = messages
        .iter()
        .enumerate()
        .filter(|(index, _)| selected[*index])
        .map(|(_, message)| message_bytes(message))
        .sum::<usize>();
    let mut count = selected.iter().filter(|selected| **selected).count();
    let message_limit = MAX_CONTEXT_MESSAGES - 1;
    for unit in units.iter().rev() {
        if unit.clone().any(|index| selected[index]) {
            continue;
        }
        let unit_bytes = messages[unit.clone()]
            .iter()
            .map(message_bytes)
            .sum::<usize>();
        let next_count = count + unit.len();
        if bytes + unit_bytes > byte_budget || next_count > message_limit {
            continue;
        }
        for message in unit.clone() {
            selected[message] = true;
            if messages[message].role != MessageRole::System {
                count += 1;
                bytes += message_bytes(&messages[message]);
            }
        }
    }

    let mut bounded = messages
        .iter()
        .enumerate()
        .filter(|(index, _)| selected[*index])
        .map(|(_, message)| message.clone())
        .collect::<Vec<_>>();
    let omitted = selected.iter().filter(|selected| !**selected).count();
    if omitted > 0 {
        let notice = CONTEXT_NOTICE.replace("{omitted}", &omitted.to_string());
        let insert_at = bounded
            .iter()
            .take_while(|message| message.role == MessageRole::System)
            .count();
        bounded.insert(insert_at, ChatMessage::system(notice));
        if bounded.len() > MAX_CONTEXT_MESSAGES
            || bounded.iter().map(message_bytes).sum::<usize>() > MAX_CONTEXT_BYTES
        {
            return Err("Bounded context exceeded its message or byte limit".into());
        }
    }
    Ok(bounded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(id: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            name: "read_file".into(),
            arguments: format!("{{\"path\":\"{id}.rs\"}}"),
        }
    }

    fn assistant(content: &str) -> ChatMessage {
        ChatMessage {
            role: MessageRole::Assistant,
            content: content.into(),
            reasoning_content: None,
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    fn tool_response(turn: &mut AgentTurn, calls: Vec<ToolCall>) {
        turn.begin_response();
        turn.push_reasoning("Kimi chain of thought");
        turn.push_content("Inspecting");
        turn.push_tool_calls(calls);
        assert_eq!(turn.finish_response(), ResponseEnd::ToolCalls);
    }

    #[test]
    fn mock_tool_result_followup_preserves_reasoning_and_call_id() {
        let mut turn = AgentTurn::default();
        turn.begin(vec![ChatMessage::user("Inspect the project")]);
        tool_response(&mut turn, vec![call("call-1")]);

        let NextTool::Execute(active) = turn.next_tool() else {
            panic!("tool was not selected");
        };
        turn.complete_tool(&active.id, "mock file contents".into())
            .unwrap();
        assert_eq!(turn.next_tool(), NextTool::FollowUp);

        let messages = turn.request_messages().unwrap();
        let assistant = &messages[1];
        assert_eq!(assistant.role, MessageRole::Assistant);
        assert_eq!(
            assistant.reasoning_content.as_deref(),
            Some("Kimi chain of thought")
        );
        assert_eq!(assistant.tool_calls[0].id, "call-1");
        assert_eq!(
            messages[2],
            ChatMessage::tool_result("call-1", "mock file contents")
        );
        assert_eq!(turn.finish_response(), ResponseEnd::Ignored);
    }

    #[test]
    fn mock_streamed_final_text_is_recorded_once() {
        let mut turn = AgentTurn::default();
        turn.begin(vec![ChatMessage::user("Explain this code")]);
        turn.begin_response();
        turn.push_reasoning("Reasoning ");
        turn.push_reasoning("complete");
        turn.push_content("Final ");
        turn.push_content("answer");

        assert_eq!(turn.finish_response(), ResponseEnd::Final);
        assert_eq!(turn.finish_response(), ResponseEnd::Ignored);

        let messages = turn.request_messages().unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[1].role, MessageRole::Assistant);
        assert_eq!(messages[1].content, "Final answer");
        assert_eq!(
            messages[1].reasoning_content.as_deref(),
            Some("Reasoning complete")
        );
        assert!(messages[1].tool_calls.is_empty());
    }

    #[test]
    fn mock_multiple_calls_run_one_at_a_time_in_order() {
        let mut turn = AgentTurn::default();
        turn.begin(vec![ChatMessage::user("Inspect two files")]);
        tool_response(&mut turn, vec![call("call-1"), call("call-2")]);

        let NextTool::Execute(first) = turn.next_tool() else {
            panic!("first tool missing");
        };
        assert_eq!(first.id, "call-1");
        assert!(turn.complete_tool("call-2", "wrong result".into()).is_err());
        turn.complete_tool("call-1", "first result".into()).unwrap();

        let NextTool::Execute(second) = turn.next_tool() else {
            panic!("second tool missing");
        };
        assert_eq!(second.id, "call-2");
        turn.complete_tool("call-2", "second result".into())
            .unwrap();
        assert_eq!(turn.next_tool(), NextTool::FollowUp);

        let messages = turn.request_messages().unwrap();
        let tool_ids = messages
            .iter()
            .filter_map(|message| message.tool_call_id.as_deref())
            .collect::<Vec<_>>();
        assert_eq!(tool_ids, ["call-1", "call-2"]);
        assert_eq!(messages[1].tool_calls.len(), 2);
    }

    #[test]
    fn mock_denial_and_tool_error_are_correlated_results() {
        for result in ["Action denied by user", "Tool error: read failed"] {
            let mut turn = AgentTurn::default();
            turn.begin(vec![ChatMessage::user("Try a tool")]);
            tool_response(&mut turn, vec![call("call-denied")]);
            let NextTool::Execute(active) = turn.next_tool() else {
                panic!("tool missing");
            };
            turn.complete_tool(&active.id, result.into()).unwrap();
            assert_eq!(turn.next_tool(), NextTool::FollowUp);
            let messages = turn.request_messages().unwrap();
            assert_eq!(
                messages.last().unwrap().tool_call_id.as_deref(),
                Some("call-denied")
            );
            assert_eq!(messages.last().unwrap().content, result);
        }
    }

    #[test]
    fn many_tool_calls_continue_without_a_step_limit() {
        let mut turn = AgentTurn::default();
        turn.begin(vec![ChatMessage::user("First turn")]);
        for index in 0..25 {
            tool_response(&mut turn, vec![call(&format!("call-{index}"))]);
            let NextTool::Execute(active) = turn.next_tool() else {
                panic!("tool missing");
            };
            turn.complete_tool(&active.id, "ok".into()).unwrap();
            assert_eq!(turn.next_tool(), NextTool::FollowUp);
        }
        turn.begin(vec![ChatMessage::user("Independent second turn")]);
        tool_response(&mut turn, vec![call("new-turn")]);
        assert!(matches!(turn.next_tool(), NextTool::Execute(_)));
    }

    #[test]
    fn small_multi_turn_context_is_returned_unchanged() {
        let messages = vec![
            ChatMessage::system("Keep the project constraints in mind."),
            ChatMessage::user("Inspect the parser."),
            assistant("It tokenizes source text."),
            ChatMessage::user("Add a regression test."),
            assistant("The test now covers escapes."),
        ];

        assert_eq!(bounded_context(&messages).unwrap(), messages);
    }

    #[test]
    fn overflow_keeps_the_latest_goal_and_recent_history_before_it() {
        let mut messages = vec![ChatMessage::system("Always report failures honestly.")];
        for index in 0..60 {
            messages.push(ChatMessage::user(format!("older request {index}")));
        }
        messages.push(ChatMessage::user("latest user goal"));

        let bounded = bounded_context(&messages).unwrap();
        assert!(bounded.len() <= MAX_CONTEXT_MESSAGES);
        assert!(bounded.iter().any(|message| {
            message.role == MessageRole::User && message.content == "latest user goal"
        }));
        assert!(bounded.iter().any(|message| {
            message.role == MessageRole::User && message.content == "older request 59"
        }));
        assert!(!bounded.iter().any(|message| {
            message.role == MessageRole::User && message.content == "older request 0"
        }));
        let positions = bounded
            .iter()
            .enumerate()
            .filter(|(_, message)| message.role == MessageRole::User)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        assert!(positions.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn an_oversized_latest_tool_exchange_is_an_error_not_a_silent_drop() {
        let call = ToolCall {
            id: "huge-call".into(),
            name: "write_file".into(),
            arguments: "x".repeat(MAX_CONTEXT_BYTES + 1),
        };
        let messages = vec![
            ChatMessage::system("Keep this system instruction."),
            ChatMessage::user("Write the file."),
            ChatMessage::assistant_tool_calls(String::new(), String::new(), &[call]),
            ChatMessage::tool_result("huge-call", "ok"),
        ];

        let error = bounded_context(&messages).unwrap_err();
        assert!(error.contains("latest complete tool exchange"), "{error}");
    }

    #[test]
    fn context_is_bounded_without_orphaning_tool_results() {
        let mut turn = AgentTurn::default();
        turn.begin(
            (0..40)
                .map(|index| {
                    ChatMessage::user(format!("old message {index}: {}", "x".repeat(20_000)))
                })
                .collect(),
        );
        tool_response(&mut turn, vec![call("latest")]);
        let NextTool::Execute(active) = turn.next_tool() else {
            panic!("tool missing");
        };
        turn.complete_tool(&active.id, "x".repeat(MAX_TOOL_RESULT_BYTES * 2))
            .unwrap();

        let messages = turn.request_messages().unwrap();
        assert!(messages.len() <= MAX_CONTEXT_MESSAGES);
        assert!(messages.iter().map(message_bytes).sum::<usize>() <= MAX_CONTEXT_BYTES);
        assert_eq!(messages[0].role, MessageRole::System);
        assert!(
            messages[0]
                .content
                .contains("Project inspection may be incomplete")
        );
        let assistant = messages
            .iter()
            .position(|message| !message.tool_calls.is_empty())
            .unwrap();
        assert_eq!(
            messages[assistant + 1].tool_call_id.as_deref(),
            Some("latest")
        );
        assert!(
            messages[assistant + 1]
                .content
                .contains("tool result truncated")
        );
    }

    #[test]
    fn cancellation_clears_pending_work() {
        let mut turn = AgentTurn::default();
        turn.begin(vec![ChatMessage::user("Inspect")]);
        tool_response(&mut turn, vec![call("one"), call("two")]);
        let NextTool::Execute(active) = turn.next_tool() else {
            panic!("tool missing");
        };
        turn.cancel();
        assert!(
            turn.complete_tool(&active.id, "late result".into())
                .is_err()
        );
        assert_eq!(turn.next_tool(), NextTool::FollowUp);
        assert_eq!(turn.finish_response(), ResponseEnd::Ignored);
    }

    #[test]
    fn goal_and_system_instructions_survive_more_than_48_tool_messages() {
        let mut turn = AgentTurn::default();
        turn.begin(vec![
            ChatMessage::system("Always verify the implementation before reporting success."),
            ChatMessage::system("Project instructions: inspect bounded results honestly."),
            ChatMessage::user("Implement the complete requested feature and report blockers."),
        ]);
        for index in 0..80 {
            tool_response(&mut turn, vec![call(&format!("call-{index}"))]);
            let NextTool::Execute(active) = turn.next_tool() else {
                panic!("tool missing at {index}");
            };
            turn.complete_tool(&active.id, "x".repeat(8_000)).unwrap();
            assert_eq!(turn.next_tool(), NextTool::FollowUp);
        }

        let messages = turn.request_messages().unwrap();
        assert!(messages.len() <= MAX_CONTEXT_MESSAGES);
        assert!(
            messages
                .iter()
                .any(|message| message.role == MessageRole::System
                    && message.content.contains("Always verify"))
        );
        assert!(
            messages.iter().any(|message| {
                message.role == MessageRole::User
                    && message.content.contains("complete requested feature")
            }),
            "the original user goal was dropped"
        );
        assert!(
            messages
                .iter()
                .any(|message| message.role == MessageRole::System
                    && message.content.contains("older messages were omitted"))
        );
        for (index, message) in messages.iter().enumerate() {
            if message.role != MessageRole::Tool {
                continue;
            }
            assert!(
                messages[..index].iter().any(|candidate| candidate
                    .tool_calls
                    .iter()
                    .any(|call| Some(call.id.as_str()) == message.tool_call_id.as_deref())),
                "orphan tool result at {index}"
            );
        }
    }

    #[test]
    fn replayed_tool_call_ids_are_executed_only_once() {
        let mut turn = AgentTurn::default();
        turn.begin(vec![ChatMessage::user("Do the work")]);
        turn.begin_response();
        turn.push_tool_calls(vec![call("same-call"), call("same-call")]);
        assert_eq!(turn.finish_response(), ResponseEnd::ToolCalls);
        let NextTool::Execute(first) = turn.next_tool() else {
            panic!("tool missing");
        };
        assert_eq!(first.id, "same-call");
        turn.complete_tool(&first.id, "done".into()).unwrap();
        assert_eq!(turn.next_tool(), NextTool::FollowUp);

        turn.begin_response();
        turn.push_tool_calls(vec![call("same-call")]);
        assert_eq!(turn.finish_response(), ResponseEnd::Final);
    }
}
