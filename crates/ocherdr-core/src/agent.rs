//! Agent-facing rules shared by the GUI. Herdr remains authoritative; these
//! helpers exist so the client can reject illegal names before a round-trip
//! and decide when to refresh `agent.read` without polling.

use super::HerdrEvent;

/// Herdr: `[a-z][a-z0-9_-]{0,31}` — one to 32 bytes.
const AGENT_NAME_MAX_LEN: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentNameError {
    FirstCharacter,
    InvalidCharacter,
    TooLong,
}

/// Parse a custom agent name. An exactly empty input means clear the custom
/// name by omitting `name` from `agent.rename`.
pub fn parse_agent_name(raw: &str) -> Result<Option<&str>, AgentNameError> {
    if raw.is_empty() {
        return Ok(None);
    }
    let bytes = raw.as_bytes();
    if !bytes[0].is_ascii_lowercase() {
        return Err(AgentNameError::FirstCharacter);
    }
    if bytes.len() > AGENT_NAME_MAX_LEN {
        return Err(AgentNameError::TooLong);
    }
    if bytes[1..].iter().any(|byte| {
        !byte.is_ascii_lowercase() && !byte.is_ascii_digit() && *byte != b'-' && *byte != b'_'
    }) {
        return Err(AgentNameError::InvalidCharacter);
    }
    Ok(Some(raw))
}

/// CLI and Herdr's own default for `agent.read`. Visible is only the current
/// viewport; detection text is an internal matcher; recent-unwrapped is for
/// parsers. Recent is the last ~80 lines as shown, which is what a panel
/// "recent output" should display.
pub const AGENT_OUTPUT_SOURCE: &str = "recent";

pub fn agent_output_should_refresh(panel_pane: &str, event: &HerdrEvent) -> bool {
    match event {
        HerdrEvent::PaneAgentStatusChanged { pane_id, .. } => pane_id == panel_pane,
        HerdrEvent::PaneAgentDetected { pane_id, .. } => pane_id == panel_pane,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentStatus;
    use std::collections::HashMap;

    fn status_event(pane_id: &str) -> HerdrEvent {
        HerdrEvent::PaneAgentStatusChanged {
            pane_id: pane_id.into(),
            workspace_id: "w".into(),
            agent_status: AgentStatus::Working,
            agent: Some("grok".into()),
            title: None,
            display_agent: Some("grok".into()),
            state_labels: HashMap::new(),
        }
    }

    fn detected_event(pane_id: &str) -> HerdrEvent {
        HerdrEvent::PaneAgentDetected {
            workspace_id: "w".into(),
            pane_id: pane_id.into(),
            agent: Some("grok".into()),
            released: false,
            final_status: None,
        }
    }

    #[test]
    fn accepted_agent_names_match_herdr() {
        for name in ["grok", "a", "x_1-2"] {
            assert_eq!(parse_agent_name(name), Ok(Some(name)), "{name}");
        }
    }

    #[test]
    fn agent_names_must_start_with_a_lowercase_letter() {
        for name in ["1abc", "Abc", "_x"] {
            assert_eq!(
                parse_agent_name(name),
                Err(AgentNameError::FirstCharacter),
                "{name}"
            );
        }
    }

    #[test]
    fn agent_names_reject_characters_outside_the_herdr_alphabet() {
        for name in ["a.b", "a b", "a@b"] {
            assert_eq!(
                parse_agent_name(name),
                Err(AgentNameError::InvalidCharacter),
                "{name}"
            );
        }
    }

    #[test]
    fn agent_name_length_limit_is_32_bytes() {
        let thirty_two = format!("a{}", "b".repeat(31));
        let thirty_three = format!("a{}", "b".repeat(32));
        assert_eq!(thirty_two.len(), 32);
        assert_eq!(thirty_three.len(), 33);
        assert_eq!(parse_agent_name(&thirty_two), Ok(Some(thirty_two.as_str())));
        assert_eq!(
            parse_agent_name(&thirty_three),
            Err(AgentNameError::TooLong)
        );
    }

    #[test]
    fn an_empty_agent_name_clears_the_custom_name() {
        assert_eq!(parse_agent_name(""), Ok(None));
    }

    #[test]
    fn agent_names_reject_leading_trailing_and_only_whitespace() {
        assert_eq!(
            parse_agent_name(" reviewer"),
            Err(AgentNameError::FirstCharacter)
        );
        assert_eq!(
            parse_agent_name("reviewer "),
            Err(AgentNameError::InvalidCharacter)
        );
        assert_eq!(parse_agent_name("   "), Err(AgentNameError::FirstCharacter));
    }

    #[test]
    fn agent_output_refreshes_only_for_the_open_pane_status_or_detect() {
        assert!(agent_output_should_refresh("p1", &status_event("p1")));
        assert!(agent_output_should_refresh("p1", &detected_event("p1")));
        assert!(!agent_output_should_refresh("p1", &status_event("p2")));
        assert!(!agent_output_should_refresh(
            "p1",
            &HerdrEvent::PaneFocused {
                workspace_id: "w".into(),
                pane_id: "p1".into(),
            }
        ));
    }
}
