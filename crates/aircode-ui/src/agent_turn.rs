use std::{collections::VecDeque, ops::Range};

use aircode_ai::{ChatMessage, MessageRole, ToolCall};

pub const MAX_AGENT_STEPS: usize = 8;
const MAX_TOOL_RESULT_BYTES: usize = 32_000;
const MAX_CONTEXT_MESSAGES: usize = 48;
const MAX_CONTEXT_BYTES: usize = 500_000;
const CONTEXT_NOTICE: &str = "Context truncated: older messages were omitted. Project inspection may be incomplete; do not claim the entire project was scanned.";

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
    StepLimit,
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
    steps: usize,
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
            self.response.calls.extend(calls);
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
            return NextTool::StepLimit;
        }
        let Some(call) = self.pending.pop_front() else {
            return NextTool::FollowUp;
        };
        if self.steps >= MAX_AGENT_STEPS {
            self.pending.push_front(call);
            return NextTool::StepLimit;
        }
        self.steps += 1;
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

    /// Start another bounded batch without losing pending tool calls or their results.
    pub fn resume(&mut self) -> bool {
        if self.active.is_some() || self.pending.is_empty() {
            return false;
        }
        self.steps = 0;
        true
    }

    pub fn request_messages(&self) -> Result<Vec<ChatMessage>, String> {
        bounded_context(&self.messages)
    }

    #[cfg(test)]
    fn steps(&self) -> usize {
        self.steps
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

/// Keep a contiguous suffix and never separate an assistant call from its results.
fn bounded_context(messages: &[ChatMessage]) -> Result<Vec<ChatMessage>, String> {
    if messages.is_empty() {
        return Ok(Vec::new());
    }
    let units = context_units(messages);
    let reserved = CONTEXT_NOTICE.len() + 32;
    let byte_budget = MAX_CONTEXT_BYTES - reserved;
    let message_budget = MAX_CONTEXT_MESSAGES - 1;
    let mut first = units.len();
    let mut bytes = 0;
    let mut count = 0;

    for (index, unit) in units.iter().enumerate().rev() {
        let unit_bytes = messages[unit.clone()]
            .iter()
            .map(message_bytes)
            .sum::<usize>();
        let next_count = count + unit.len();
        if bytes + unit_bytes > byte_budget || next_count > message_budget {
            if first == units.len() {
                return Err("The latest tool exchange exceeds the context limit".into());
            }
            break;
        }
        first = index;
        bytes += unit_bytes;
        count = next_count;
    }

    let start = units[first].start;
    let mut bounded = messages[start..].to_vec();
    if start > 0 {
        bounded.insert(0, ChatMessage::system(CONTEXT_NOTICE));
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
    fn step_limit_resets_for_each_user_turn() {
        let mut turn = AgentTurn::default();
        turn.begin(vec![ChatMessage::user("First turn")]);
        for index in 0..MAX_AGENT_STEPS {
            tool_response(&mut turn, vec![call(&format!("call-{index}"))]);
            let NextTool::Execute(active) = turn.next_tool() else {
                panic!("tool missing");
            };
            turn.complete_tool(&active.id, "ok".into()).unwrap();
            assert_eq!(turn.next_tool(), NextTool::FollowUp);
        }
        assert_eq!(turn.steps(), MAX_AGENT_STEPS);
        tool_response(&mut turn, vec![call("over-limit")]);
        assert_eq!(turn.next_tool(), NextTool::StepLimit);

        assert!(turn.resume());
        let NextTool::Execute(resumed) = turn.next_tool() else {
            panic!("pending call lost")
        };
        assert_eq!(resumed.id, "over-limit");
        turn.complete_tool(&resumed.id, "resumed".into()).unwrap();
        assert_eq!(turn.next_tool(), NextTool::FollowUp);
        assert_eq!(
            turn.request_messages()
                .unwrap()
                .last()
                .unwrap()
                .tool_call_id
                .as_deref(),
            Some("over-limit")
        );

        turn.begin(vec![ChatMessage::user("Independent second turn")]);
        assert_eq!(turn.steps(), 0);
        tool_response(&mut turn, vec![call("new-turn")]);
        assert!(matches!(turn.next_tool(), NextTool::Execute(_)));
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
}
