//! What Cloud kept of an identity sync. Cloud answers 200 even when it keeps
//! none of the identities, and it holds this installation's attestation key
//! only while at least one identity verifies, so the log names each skipped
//! agent and Cloud's reason. Tokens and key material are never logged.
use serde_json::Value;

/// One line: the identity count sent, whether the attestation key went with
/// them, Cloud's `synced` answer, and each skipped agent id with its reason.
pub(super) fn describe(body: &Value, response: &Value) -> String {
    let sent = body["identities"].as_array().map_or(0, Vec::len);
    let key = if body.get("attestation_key").is_some() {
        "with"
    } else {
        "without"
    };
    let synced = response
        .get("synced")
        .map_or("absent".into(), Value::to_string);
    let skipped: Vec<String> = response["skipped"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|entry| {
            let field = |name: &str| entry[name].as_str().unwrap_or("?").to_owned();
            format!("{} ({})", field("agent_id"), field("reason"))
        })
        .collect();
    let skipped = if skipped.is_empty() {
        "none".to_owned()
    } else {
        skipped.join(", ")
    };
    format!(
        "[desktop-identity] sent {sent} identities {key} the attestation key; \
         Cloud synced: {synced}; skipped: {skipped}"
    )
}

/// To stderr and the core's `core.log`, which launchd does not discard.
pub(super) fn log(body: &Value, response: &Value) {
    let line = describe(body, response);
    agent_relay_node::relay::diagnostics::write(format_args!("{line}"));
}

#[cfg(test)]
mod tests {
    use super::describe;
    use serde_json::json;

    #[test]
    fn a_skipped_sync_names_each_agent_and_reason_without_secrets() {
        let body = json!({
            "source_id": "source",
            "identities": [
                {"name": "a", "agent_id": "agent-a", "token": "secret-token-a"},
                {"name": "b", "agent_id": "agent-b", "token": "secret-token-b"},
            ],
            "attestation_key": {"kid": "kid", "public_key": "secret-key-material"},
        });
        let response = json!({"synced": 0, "skipped": [
            {"agent_id": "agent-a", "reason": "unverified"},
            {"agent_id": "agent-b", "reason": "not_owned"},
        ]});
        let line = describe(&body, &response);
        assert_eq!(
            line,
            "[desktop-identity] sent 2 identities with the attestation key; \
             Cloud synced: 0; skipped: agent-a (unverified), agent-b (not_owned)"
        );
        assert!(!line.contains("secret"));
    }

    #[test]
    fn an_empty_sync_without_a_key_says_so() {
        let line = describe(
            &json!({"source_id": "s", "identities": []}),
            &json!({"synced": true}),
        );
        assert_eq!(
            line,
            "[desktop-identity] sent 0 identities without the attestation key; \
             Cloud synced: true; skipped: none"
        );
    }
}
