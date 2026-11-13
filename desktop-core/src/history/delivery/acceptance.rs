//! How far a receiver has taken an acknowledged batch.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AcceptanceLevel {
    Durable,
    Indexed,
    /// Stored in the workspace's own storage and waiting for the receiver to
    /// admit it. The batch is settled: the receiver takes it from there.
    Queued,
}

impl AcceptanceLevel {
    /// The name the job's status stores and reports.
    pub fn name(self) -> &'static str {
        match self {
            Self::Durable => "durable",
            Self::Indexed => "indexed",
            Self::Queued => "queued",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_level_is_named_as_it_is_serialised() {
        for level in [
            AcceptanceLevel::Durable,
            AcceptanceLevel::Indexed,
            AcceptanceLevel::Queued,
        ] {
            assert_eq!(
                serde_json::to_value(level).unwrap(),
                serde_json::Value::from(level.name())
            );
        }
    }
}
