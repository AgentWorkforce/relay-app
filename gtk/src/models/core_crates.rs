//! The core crates this build was compiled with, for About's diagnostics.

/// The core crates this build was compiled with: the commit of
/// agent-relay-node and the version of relaycast that `gtk/Cargo.lock`
/// resolved, the pins or an internal build's overrides
/// (`scripts/apply-crate-overrides.sh`). `build.rs` writes them to
/// `$OUT_DIR/crates.json` in the shape the script's `--json` writes, the
/// file macOS reads from its bundle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CoreCrates {
    pub agent_relay_node: String,
    pub relaycast: String,
}

impl CoreCrates {
    /// `{"agent-relay-node":{"rev":SHA},"relaycast":{"version":V}}`; anything
    /// else is no record.
    pub fn parse(json: &str) -> Option<Self> {
        let value: serde_json::Value = serde_json::from_str(json).ok()?;
        let text = |crate_name: &str, field: &str| {
            value
                .get(crate_name)?
                .get(field)?
                .as_str()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
        };
        Some(Self {
            agent_relay_node: text("agent-relay-node", "rev")?,
            relaycast: text("relaycast", "version")?,
        })
    }

    /// The crates this app was built with.
    pub fn built() -> Option<Self> {
        Self::parse(include_str!(concat!(env!("OUT_DIR"), "/crates.json")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_core_crates_record_reads_as_build_rs_writes_it() {
        let crates = CoreCrates::parse(
            r#"{"agent-relay-node":{"rev":"0123456789abcdef0123456789abcdef01234567"},"relaycast":{"version":"10.0.1"}}"#,
        )
        .unwrap();
        assert_eq!(
            crates,
            CoreCrates {
                agent_relay_node: "0123456789abcdef0123456789abcdef01234567".into(),
                relaycast: "10.0.1".into(),
            }
        );
        for malformed in [
            "",
            "{}",
            r#"{"agent-relay-node":{"rev":""},"relaycast":{"version":"10.0.1"}}"#,
            r#"{"agent-relay-node":"abc","relaycast":{"version":"10.0.1"}}"#,
        ] {
            assert_eq!(CoreCrates::parse(malformed), None, "{malformed}");
        }
        // This build's own record, from gtk/Cargo.lock, is well formed.
        let built = CoreCrates::built().expect("build.rs records the crates");
        assert_eq!(built.agent_relay_node.len(), 40);
        assert!(!built.relaycast.is_empty());
    }
}
