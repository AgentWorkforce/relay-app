//! A task the cloud Skip lead describes in a reply: the reply's last line
//! is `SKIP_APP_TASK {json}`, drawn as a task card under the rest of the
//! reply. Display only: nothing in it is taken as leave to contact or
//! release an agent. Skip.app's `RelayAppTaskBreakdown` reads the same line
//! with the same limits.

use serde::Deserialize;

/// The Skip lead's name on the relay.
pub const SKIP_LEAD: &str = "skip";
const PREFIX: &str = "SKIP_APP_TASK ";
const MAX_LINE: usize = 8_192;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkipTaskAgent {
    pub name: String,
    pub address: Option<String>,
    pub assignment: Option<String>,
    pub summary: Option<String>,
    pub machine: Option<String>,
    pub model: Option<String>,
    pub harness: Option<String>,
    pub started_at: Option<String>,
    /// `live`, `offline` or `unknown`.
    pub lifecycle: Option<String>,
    pub evidence: Option<String>,
}

impl SkipTaskAgent {
    /// How long it has run, while it is live: `12m`, `3h 4m`, `2d 5h`.
    pub fn active_for(&self, now_ms: i64) -> Option<String> {
        if self.lifecycle.as_deref() != Some("live") {
            return None;
        }
        let start = chrono::DateTime::parse_from_rfc3339(self.started_at.as_deref()?).ok()?;
        let elapsed = now_ms - start.timestamp_millis();
        if elapsed < 0 {
            return None;
        }
        let minutes = elapsed / 60_000;
        let hours = minutes / 60;
        Some(if minutes < 60 {
            format!("{minutes}m")
        } else if hours < 24 {
            format!("{hours}h {}m", minutes % 60)
        } else {
            format!("{}d {}h", hours / 24, hours % 24)
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkipTask {
    pub version: u32,
    pub task_id: String,
    pub title: String,
    pub state: String,
    pub owner: String,
    pub next_action: Option<String>,
    pub blocker: Option<String>,
    pub next_check_at: Option<String>,
    pub observed_at: String,
    pub agents: Vec<SkipTaskAgent>,
    pub agents_omitted: u32,
}

impl SkipTask {
    /// The reply without its task line, and the task when the last
    /// non-blank line is a valid one. Anything else leaves the reply whole.
    pub fn parse_reply(text: &str) -> (String, Option<SkipTask>) {
        let lines: Vec<&str> = text.split('\n').collect();
        let Some(index) = lines.iter().rposition(|line| !line.trim().is_empty()) else {
            return (text.to_string(), None);
        };
        let line = lines[index];
        let task = line
            .strip_prefix(PREFIX)
            .filter(|_| line.len() <= MAX_LINE)
            .and_then(|json| serde_json::from_str::<SkipTask>(json).ok())
            .filter(SkipTask::valid);
        let Some(task) = task else {
            return (text.to_string(), None);
        };
        let mut visible = lines;
        visible.remove(index);
        (visible.join("\n").trim().to_string(), Some(task))
    }

    fn valid(&self) -> bool {
        let mut names: Vec<String> = self.agents.iter().map(|a| a.name.to_lowercase()).collect();
        names.sort();
        names.dedup();
        self.version == 1
            && task_id(&self.task_id)
            && field(&self.title, 120)
            && field(&self.state, 40)
            && field(&self.owner, 100)
            && field(&self.observed_at, 40)
            && optional(&self.next_action, 200)
            && optional(&self.blocker, 200)
            && optional(&self.next_check_at, 40)
            && self.agents.len() <= 30
            && self.agents_omitted <= 10_000
            && names.len() == self.agents.len()
            && self.agents.iter().all(|agent| {
                field(&agent.name, 100)
                    && optional(&agent.address, 220)
                    && optional(&agent.assignment, 160)
                    && optional(&agent.summary, 240)
                    && optional(&agent.machine, 100)
                    && optional(&agent.model, 100)
                    && optional(&agent.harness, 60)
                    && optional(&agent.started_at, 40)
                    && optional(&agent.evidence, 150)
                    && agent
                        .lifecycle
                        .as_deref()
                        .is_none_or(|l| ["live", "offline", "unknown"].contains(&l))
            })
    }
}

/// `T` and a number without a leading zero.
fn task_id(id: &str) -> bool {
    id.strip_prefix('T').is_some_and(|digits| {
        !digits.is_empty() && !digits.starts_with('0') && digits.chars().all(|c| c.is_ascii_digit())
    })
}

fn field(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.chars().count() <= max && !value.chars().any(char::is_control)
}

fn optional(value: &Option<String>, max: usize) -> bool {
    value.as_deref().is_none_or(|value| field(value, max))
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str = r#"SKIP_APP_TASK {"version":1,"taskId":"T12","title":"Ship unified chat","state":"in_progress","owner":"khaliq","nextAction":"Review the GTK slice","observedAt":"2026-11-27T10:00:00Z","agents":[{"name":"gtk-dev","machine":"linuyc","model":"opus","harness":"claude","startedAt":"2026-11-27T08:30:00Z","lifecycle":"live"}],"agentsOmitted":2}"#;

    #[test]
    fn a_valid_last_line_becomes_the_task() {
        let (message, task) = SkipTask::parse_reply(&format!("On track.\n\n{LINE}\n"));
        let task = task.unwrap();
        assert_eq!(message, "On track.");
        assert_eq!(task.task_id, "T12");
        assert_eq!(task.agents_omitted, 2);
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-27T10:05:00Z")
            .unwrap()
            .timestamp_millis();
        assert_eq!(task.agents[0].active_for(now).as_deref(), Some("1h 35m"));
    }

    #[test]
    fn anything_else_leaves_the_reply_whole() {
        for text in [
            "No task here.".to_string(),
            format!("{LINE}\nmore after it"),
            LINE.replace("\"version\":1", "\"version\":2"),
            LINE.replace("T12", "T012"),
            LINE.replace("\"lifecycle\":\"live\"", "\"lifecycle\":\"gone\""),
            LINE.replace("\"title\":\"Ship unified chat\"", "\"title\":\" \""),
        ] {
            let (message, task) = SkipTask::parse_reply(&text);
            assert!(task.is_none(), "{text}");
            assert_eq!(message, text);
        }
    }

    #[test]
    fn only_a_live_agent_with_a_past_start_has_a_running_time() {
        let mut agent: SkipTaskAgent = serde_json::from_str(
            r#"{"name":"a","startedAt":"2026-11-27T10:00:00Z","lifecycle":"offline"}"#,
        )
        .unwrap();
        let at = |s: &str| {
            chrono::DateTime::parse_from_rfc3339(s)
                .unwrap()
                .timestamp_millis()
        };
        assert_eq!(agent.active_for(at("2026-11-27T10:10:00Z")), None);
        agent.lifecycle = Some("live".into());
        assert_eq!(agent.active_for(at("2026-11-27T09:00:00Z")), None);
        assert_eq!(
            agent.active_for(at("2026-11-29T13:00:00Z")).as_deref(),
            Some("2d 3h")
        );
    }
}
