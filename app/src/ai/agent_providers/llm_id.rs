//! `LLMId` prefix codec for BYOP (Bring Your Own Provider).
//!
//! Custom Agent provider models are distinguished by prefixing `byop:` in the `LLMId` string,
//! This allows the controller to determine whether to use the warp backend or the user's own OpenAI compatible endpoint at the request exit.
//!
//! Encoding format: `byop:<provider_id>:<model_id>`
//! - `provider_id` is `AgentProvider.id`(UUID)
//! - `model_id` is `AgentProviderModel.id` (the `model` field value sent to the upstream API)
//!
//! Example: `byop:6f3b...:deepseek-chat`
//!
//! `provider_id` is UUID without colon, `model_id` may contain colon (some upstreams have `vendor:model` style naming),
//! Therefore, split is only performed at the first colon.

use ai::LLMId;

pub const BYOP_PREFIX: &str = "byop:";

/// Encode `(provider_id, model_id)` into a single `LLMId`.
pub fn encode(provider_id: &str, model_id: &str) -> LLMId {
    LLMId::from(format!("{BYOP_PREFIX}{provider_id}:{model_id}"))
}

/// If `LLMId` is BYOP encoding, return `(provider_id, model_id)`, otherwise return `None`.
pub fn decode(id: &LLMId) -> Option<(String, String)> {
    let s = id.as_str().strip_prefix(BYOP_PREFIX)?;
    let (pid, mid) = s.split_once(':')?;
    if pid.is_empty() || mid.is_empty() {
        return None;
    }
    Some((pid.to_owned(), mid.to_owned()))
}

/// Is this `LLMId` BYOP encoding (for the caller to quickly determine when there is no need to split fields).
pub fn is_byop(id: &LLMId) -> bool {
    id.as_str().starts_with(BYOP_PREFIX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let id = encode("uuid-123", "deepseek-chat");
        assert_eq!(id.as_str(), "byop:uuid-123:deepseek-chat");
        assert_eq!(
            decode(&id),
            Some(("uuid-123".to_owned(), "deepseek-chat".to_owned()))
        );
    }

    #[test]
    fn model_id_with_colon_is_preserved() {
        // For example, OpenRouter's "anthropic/claude-3-haiku" does not contain a colon,
        // But some gateways may use "vendor:model:variant". We only split at the first colon,
        // The remaining part is used as model_id.
        let id = encode("uuid-1", "vendor:model:v2");
        assert_eq!(
            decode(&id),
            Some(("uuid-1".to_owned(), "vendor:model:v2".to_owned()))
        );
    }

    #[test]
    fn non_byop_returns_none() {
        let id = LLMId::from("gpt-5.2");
        assert_eq!(decode(&id), None);
        assert!(!is_byop(&id));
    }

    #[test]
    fn missing_parts_returns_none() {
        assert_eq!(decode(&LLMId::from("byop:")), None);
        assert_eq!(decode(&LLMId::from("byop:uuid")), None); // No colon
        assert_eq!(decode(&LLMId::from("byop::model")), None); // empty provider_id
        assert_eq!(decode(&LLMId::from("byop:uuid:")), None); // empty model_id
    }
}
