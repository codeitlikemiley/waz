use crate::ai::agent::ReceivedMessageInput;
use crate::ai::agent_events::AgentRunEvent;

/// Waz local build no longer pulls message content from the cloud mailbox or sends delivered receipts.
/// This type retains side-effect-free compatibility semantics for local harness bridging call sites.
#[derive(Clone)]
pub(crate) struct MessageHydrator;

impl MessageHydrator {
    pub(crate) fn new() -> Self {
        Self
    }

    pub(crate) async fn hydrate_event_for_recipient(
        &self,
        event: &AgentRunEvent,
        recipient_run_id: &str,
    ) -> Option<ReceivedMessageInput> {
        if event.event_type != "new_message" || event.run_id != recipient_run_id {
            return None;
        }

        None
    }

    pub(crate) async fn mark_messages_delivered_best_effort<'a, I>(
        &self,
        _message_ids: I,
    ) -> Vec<(String, anyhow::Error)>
    where
        I: IntoIterator<Item = &'a str>,
    {
        Vec::new()
    }
}
