//! Versioned desktop identity envelope; authoritative person properties belong to Cloud.

use super::*;

impl Telemetry {
    /// What every event carries. Attached events carry the user's groups;
    /// the rest create no person profile.
    pub(super) fn base_properties(&self, account: Option<&TelemetryAccount>) -> Map<String, Value> {
        let mut properties = Map::new();
        for (key, value) in [
            ("$lib", "agentrelay-desktop"),
            ("$lib_version", self.config.version.as_str()),
            ("$app_version", self.config.version.as_str()),
            ("app", "relay-desktop"),
            ("surface", "desktop"),
            ("emitter", "relay-desktop.core"),
            ("actor_kind", "human"),
            (
                "identity_source",
                if account.is_some() {
                    "cloud_install"
                } else {
                    "anonymous_generation"
                },
            ),
            ("platform", platform()),
            ("environment", "production"),
            ("os_version", self.os_version.as_str()),
            ("probe_version", self.config.version.as_str()),
        ] {
            properties.insert(key.into(), value.into());
        }
        properties.insert("analytics_contract_version".into(), 2.into());
        properties.insert("$geoip_disable".into(), true.into());
        match account {
            Some(account) => {
                let mut groups = Map::new();
                for (kind, id) in [
                    ("organization", &account.organization_id),
                    ("workspace", &account.workspace_id),
                ] {
                    if let Some(id) = id {
                        groups.insert(kind.into(), id.clone().into());
                    }
                }
                if !groups.is_empty() {
                    properties.insert("$groups".into(), Value::Object(groups));
                }
            }
            None => {
                properties.insert("$process_person_profile".into(), false.into());
            }
        }
        properties
    }
}

pub(super) fn operation_events(
    event: TelemetryEvent,
    capture_exception: bool,
    mut object: Map<String, Value>,
    distinct_id: &str,
    timestamp: &str,
) -> Vec<Value> {
    object.insert("operation".into(), event.operation.as_str().into());
    object.insert("outcome".into(), event.outcome.as_str().into());
    event.add_usage_properties(&mut object);
    if let Some(count) = event.count {
        object.insert(
            "session_count".into(),
            count.clamp(0, MAX_SESSION_COUNT).into(),
        );
    }
    if let Some(mode) = event.mode {
        object.insert("sharing_mode".into(), mode.as_str().into());
    }
    if let Some(failure) = event.failure {
        object.insert("error_code".into(), failure.as_str().into());
    }
    let envelope = |name: &str, properties: &Map<String, Value>| {
        json!({
            "event": name,
            "distinct_id": distinct_id,
            "timestamp": timestamp,
            "properties": properties,
        })
    };
    let mut events = vec![envelope("desktop_operation", &object)];
    if let (Some(failure), true) = (event.failure, capture_exception) {
        let (operation, code) = (event.operation.as_str(), failure.as_str());
        object.insert("$exception_level".into(), "error".into());
        object.insert(
            "$exception_fingerprint".into(),
            format!("desktop:{operation}:{code}").into(),
        );
        object.insert(
            "$exception_list".into(),
            json!([{
                "type": "DesktopOperationError",
                "value": format!("{operation}: {code}"),
                "mechanism": {"type": "handled", "handled": true, "synthetic": true},
            }]),
        );
        events.push(envelope("$exception", &object));
    }
    events
}
