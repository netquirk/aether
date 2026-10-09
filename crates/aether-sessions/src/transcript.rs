use crate::model::{SessionEvent, UserEvent};
use aether_core::events::{
    AgentEvent, ContextEvent, MessageEvent, ToolEvent, TurnEvent, TurnOutcome, refusal_context_message,
    task_created_result,
};
use llm::{
    AssistantReasoning, ChatMessage, Context, LlmCallPurpose, MessageId, ModelIdentity, ToolCallError, ToolCallResult,
};
use serde::{Deserialize, Serialize};

/// One turn in a run transcript, recording the model that served it.
///
/// A turn is anchored by its terminal [`TurnEvent::Ended`] event; the recorded
/// model is the first chat-purpose [`TurnEvent::LlmCallStarted`] between the
/// previous `Ended` (or the start of the stream) and the closing `Ended`.
/// Turns whose `LlmCallStarted` did not survive persistence, or that had no
/// chat call, are still recorded but their model fields are `None`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnEntry {
    /// 0-based index of the turn in the order it completed.
    pub turn_index: usize,
    /// The terminal outcome the agent reached for the turn.
    pub outcome: TurnOutcome,
    /// The provider name for the model that served the turn, if recorded.
    pub provider: Option<String>,
    /// The model id for the model that served the turn, if recorded.
    pub model_id: Option<String>,
    /// The display name the provider reported for the model.
    pub display_name: Option<String>,
}

/// Builds a turn-by-turn transcript view from a run's persisted events.
///
/// A turn is the run between two terminal [`TurnEvent::Ended`] events
/// (or from the start of the stream to the first `Ended`). Turns that never
/// reach an `Ended` event are dropped, because their outcome is unknown. The
/// recorded model is the first chat-purpose [`TurnEvent::LlmCallStarted`]
/// in the turn; a turn with no chat call (or none that survived
/// persistence) records `None` for its model fields.
pub fn turn_entries_from_events(events: &[SessionEvent]) -> Vec<TurnEntry> {
    let mut entries: Vec<TurnEntry> = Vec::new();
    let mut turn_index: usize = 0;
    let mut next_chat_identity: Option<&ModelIdentity> = None;
    let mut next_chat_display_name: Option<&str> = None;

    for event in events {
        if let SessionEvent::Agent(AgentEvent::Turn(TurnEvent::LlmCallStarted {
            purpose: LlmCallPurpose::Chat,
            model,
            display_name,
            ..
        })) = event
        {
            if next_chat_identity.is_none() {
                next_chat_identity = Some(model);
                next_chat_display_name = Some(display_name.as_str());
            }
            continue;
        }
        if let SessionEvent::Agent(AgentEvent::Turn(TurnEvent::Ended { outcome })) = event {
            let identity = next_chat_identity.take();
            let display_name = next_chat_display_name.take().map(str::to_string);
            entries.push(TurnEntry {
                turn_index,
                outcome: outcome.clone(),
                provider: identity.and_then(|id| id.provider.clone()),
                model_id: identity.and_then(|id| id.model_id.clone()),
                display_name,
            });
            turn_index += 1;
        }
    }
    entries
}

pub fn context_from_events(events: &[SessionEvent]) -> Context {
    let mut context = Context::new(vec![], vec![]);
    let mut acc = MessageAccumulator::default();
    for event in events {
        match event {
            SessionEvent::User(event) => {
                acc.flush(&mut context);
                apply_user_event(&mut context, event);
            }
            SessionEvent::Agent(event) => apply_agent_event(&mut context, event, &mut acc),
            SessionEvent::Control(_) => {}
        }
    }
    acc.flush(&mut context);
    context
}

pub fn conversation_messages_from_events(events: &[SessionEvent]) -> Vec<ChatMessage> {
    context_from_events(events).messages().iter().filter(|message| !message.is_system()).cloned().collect()
}

#[derive(Default)]
struct MessageAccumulator {
    message_id: Option<MessageId>,
    text: String,
    reasoning: String,
    tool_results: Vec<Result<ToolCallResult, ToolCallError>>,
    task_messages: Vec<ChatMessage>,
}

impl MessageAccumulator {
    fn flush(&mut self, context: &mut Context) {
        let pending = std::mem::take(self);
        if let Some(message_id) = pending.message_id {
            let reasoning = AssistantReasoning::from_parts(pending.reasoning, None);
            context.push_assistant_turn(message_id, &pending.text, reasoning, pending.tool_results);
        }
        for message in pending.task_messages {
            context.add_message(message);
        }
    }
}

fn apply_user_event(ctx: &mut Context, event: &UserEvent) {
    match event {
        UserEvent::Message { message_id, content, .. } => {
            ctx.add_message(ChatMessage::user_with_id(message_id.clone(), content.clone()));
        }
        UserEvent::ClearContext => ctx.clear_conversation(),
    }
}

fn apply_agent_event(ctx: &mut Context, event: &AgentEvent, acc: &mut MessageAccumulator) {
    match event {
        AgentEvent::Message(MessageEvent::Text { message_id, chunk, is_complete: true }) => {
            if acc.message_id.is_some() {
                acc.flush(ctx);
            }
            acc.message_id = Some(message_id.clone());
            acc.text.clone_from(chunk);
        }
        AgentEvent::Message(MessageEvent::Thought { message_id, chunk, is_complete: true }) => {
            if acc.message_id.as_ref() == Some(message_id) {
                acc.reasoning.clone_from(chunk);
            }
        }
        AgentEvent::Tool(ToolEvent::Call { .. }) => {
            if acc.message_id.is_some() {
                acc.flush(ctx);
            }
        }
        AgentEvent::Tool(ToolEvent::Result { result, .. }) => acc.tool_results.push(Ok(result.clone())),
        AgentEvent::Tool(ToolEvent::TaskCreated { request, task_id, .. }) => {
            acc.tool_results.push(Ok(task_created_result(request, task_id)));
        }
        AgentEvent::Tool(ToolEvent::Error { error }) => acc.tool_results.push(Err(error.clone())),
        AgentEvent::Tool(ToolEvent::Refused { request, reason }) => {
            acc.flush(ctx);
            ctx.add_message(refusal_context_message(request, reason));
        }
        AgentEvent::Turn(TurnEvent::AutoContinue { message_id, content, .. }) => {
            acc.flush(ctx);
            ctx.add_message(ChatMessage::user_with_id(message_id.clone(), content.clone()));
        }
        AgentEvent::Turn(TurnEvent::Ended { .. }) => acc.flush(ctx),
        AgentEvent::Context(ContextEvent::Cleared) => {
            ctx.clear_conversation();
            *acc = MessageAccumulator::default();
        }
        AgentEvent::Context(ContextEvent::CompactionResult { message_id, summary, .. }) => {
            acc.flush(ctx);
            *ctx = ctx.with_compacted_summary(message_id.clone(), summary);
        }
        AgentEvent::Tool(
            event @ (ToolEvent::TaskCompleted { .. } | ToolEvent::TaskFailed { .. } | ToolEvent::TaskCancelled { .. }),
        ) => {
            if let Some(message) = event.task_context_message() {
                if acc.message_id.is_none() && !acc.tool_results.is_empty() {
                    acc.task_messages.push(message);
                } else {
                    acc.flush(ctx);
                    ctx.add_message(message);
                }
            }
        }
        _ => {}
    }
}
