use agent_relay_node::relay::contract::transcript::LocalTranscriptEntry;

pub fn transcript_entry_visible(entry: &LocalTranscriptEntry) -> bool {
    entry.kind != "thinking"
}

pub fn transcript_entry_context(entry: &LocalTranscriptEntry) -> bool {
    entry.kind == "context"
        || matches!(entry.role.as_str(), "system" | "developer")
        || (entry.role == "user"
            && entry.kind == "text"
            && entry
                .text
                .starts_with("# Environment\nYou have been invoked in the following environment:"))
}

/// Layout growth follows replies; a person's scrollback keeps its position.
#[derive(Default)]
pub struct TranscriptScrollState {
    geometry: Option<(f64, f64)>,
    reading_history: bool,
}

impl TranscriptScrollState {
    pub fn at_latest(&self) -> bool {
        !self.reading_history
    }

    pub fn jump_to_latest(&mut self) {
        self.reading_history = false;
    }

    pub fn update(&mut self, value: f64, upper: f64, page_size: f64) -> Option<f64> {
        let geometry = (upper, page_size);
        let layout_changed = self.geometry != Some(geometry);
        self.geometry = Some(geometry);
        if layout_changed && !self.reading_history {
            return Some((upper - page_size).max(0.0));
        }
        self.reading_history = upper - page_size - value >= 100.0;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_text_in_assistant_replies_and_tool_results_keeps_its_role() {
        let setup = "# Environment\nYou have been invoked in the following environment:";
        for (role, kind, expected) in [
            ("user", "text", true),
            ("assistant", "text", false),
            ("user", "tool_result", false),
            ("tool_result", "text", false),
            ("system", "text", true),
            ("developer", "text", true),
        ] {
            let entry = serde_json::from_value(serde_json::json!({
                "id":"setup", "role":role,"kind":kind,"text":setup,"timestamp_ms":0,"truncated":false
            })).unwrap();
            assert_eq!(transcript_entry_context(&entry), expected, "{role} {kind}");
        }
    }

    #[test]
    fn follows_layout_growth_but_keeps_readers_in_history_until_they_jump() {
        let mut state = TranscriptScrollState::default();
        assert_eq!(state.update(0.0, 2000.0, 600.0), Some(1400.0));
        assert_eq!(state.update(1400.0, 2500.0, 600.0), Some(1900.0));
        assert_eq!(state.update(500.0, 2500.0, 600.0), None);
        assert!(!state.at_latest());
        assert_eq!(state.update(500.0, 3000.0, 600.0), None);
        state.jump_to_latest();
        assert_eq!(state.update(2400.0, 3500.0, 600.0), Some(2900.0));
        assert!(state.at_latest());
    }
}
