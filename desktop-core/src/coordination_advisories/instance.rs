//! Advisory instances. The pipeline reuses an advisory id after its 7-day
//! sweep and promotes a low advisory to high under the same id, so an id
//! alone does not name one note. Under [`CONTRACT_V3`] each row carries an
//! `incarnation` (a UUID, or null for a legacy row); an instance is
//! (account scope, session, advisory id, incarnation), and everything this
//! side keys by (the turn-end cache and receipts, the urgent journal, the
//! core's local-notice key) uses [`key`] for it. A legacy row keeps its id
//! as its key, exactly as before.

use super::receipts::Scope;
use super::*;
use sha2::{Digest, Sha256};

/// The listing contract whose rows carry an incarnation and whose acks of
/// instance rows are matched on it (`advisories/v3`). Only this exact value
/// counts: any other value, or none, is legacy, and nothing it lists is
/// urgent.
pub const CONTRACT_V3: &str = "advisories/v3";

/// Separates this hash's inputs from any other use of the same fields.
const DOMAIN: &str = "agent-relay-desktop/advisory-instance/1";

/// A row's incarnation under [`CONTRACT_V3`]: `Ok(None)` when it is null or
/// absent (a legacy row), `Ok(Some)` with the UUID in lower case, `Err` when
/// it is anything else, which drops the row: it is an instance row this
/// side cannot name, and a v1 ack must never stand in for its v2 ack.
pub(super) fn incarnation(value: &Value) -> Result<Option<String>, ()> {
    match value {
        Value::Null => Ok(None),
        Value::String(uuid) if is_uuid(uuid) => Ok(Some(uuid.to_ascii_lowercase())),
        _ => Err(()),
    }
}

/// The canonical 8-4-4-4-12 hexadecimal UUID form.
pub(super) fn is_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}

/// What a row, a turn-end receipt and an urgent receipt are keyed by: the
/// advisory id for a legacy row (`incarnation` `None`), else `n` and the 64
/// hexadecimal digits of the SHA-256 of the instance's fields, each length
/// prefixed in a fixed order, so no two instances share an input. That is
/// 65 characters of `[0-9a-z]`: the core's `local-notice-` key made of it is
/// 78, inside its 128-character plain-id bound, with no colon and no cut.
pub(super) fn key(scope: &Scope, target: &Target, id: &str, incarnation: Option<&str>) -> String {
    let Some(incarnation) = incarnation else {
        return id.to_string();
    };
    let mut hash = Sha256::new();
    let user = scope.user_id.as_deref();
    let fields = [
        Some(DOMAIN),
        Some(scope.site.as_str()),
        Some(scope.workspace_id.as_str()),
        user,
        Some(target.kind.as_str()),
        Some(target.id.as_str()),
        Some(id),
        Some(incarnation),
    ];
    for field in fields {
        // A field that is absent hashes as a tag of its own, never as an
        // empty value.
        hash.update([u8::from(field.is_some())]);
        let field = field.unwrap_or_default().as_bytes();
        hash.update((field.len() as u64).to_be_bytes());
        hash.update(field);
    }
    let digest = hash.finalize();
    let mut key = String::with_capacity(65);
    key.push('n');
    for byte in digest {
        key.push(char::from(HEX[usize::from(byte >> 4)]));
        key.push(char::from(HEX[usize::from(byte & 0xf)]));
    }
    key
}

const HEX: &[u8; 16] = b"0123456789abcdef";

impl Advisory {
    /// Keys this row as an instance of `scope`'s session `target`.
    pub(super) fn keyed(mut self, scope: &Scope, target: &Target) -> Self {
        self.key = key(scope, target, &self.id, self.incarnation.as_deref());
        self
    }
}

impl State {
    /// Whether `target`'s session was shown `advisory`'s id under its legacy
    /// key while `advisory` itself is an instance row: the instance may be
    /// the note that was shown, so it is handed out once more, labelled as
    /// possibly a repeat, and acknowledged from there.
    pub(super) fn shown_as_legacy(&self, target: &Target, advisory: &Advisory) -> bool {
        advisory.incarnation.is_some()
            && self
                .taken
                .get(target)
                .and_then(|taken| taken.get(&advisory.id))
                .is_some_and(|taken| taken.shown && taken.instance.is_none())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope() -> Scope {
        Scope {
            site: "https://control.example".into(),
            workspace_id: "example-workspace".into(),
            user_id: Some("example-user".into()),
        }
    }

    const UUID: &str = "0b9f6a52-3c1e-4d7a-9e2f-5a6b7c8d9e0f";

    /// The core's `is_plain_id`, which its local-notice key must pass.
    fn core_plain_id(id: &str) -> bool {
        !id.is_empty()
            && id.len() <= 128
            && id != "."
            && id != ".."
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    }

    #[test]
    fn an_instance_key_is_65_plain_characters_the_core_accepts_without_a_colon() {
        let target = Target::new(SessionKind::Codex, "11111111-1111-4111-8111-111111111111");
        // The longest advisory id a row may carry, with colons in it.
        let id = format!("a:{}", "b".repeat(126));
        assert!(plain_id(&id) && !core_plain_id(&id));
        let key = key(&scope(), &target, &id, Some(UUID));
        assert_eq!(key.len(), 65);
        assert!(key.starts_with('n') && !key.contains(':'));
        assert!(key[1..].bytes().all(|b| HEX.contains(&b)));
        assert!(plain_id(&key));
        let provider_key = format!("local-notice-{key}");
        assert!(key.len() <= 115 && core_plain_id(&provider_key));
    }

    #[test]
    fn a_legacy_row_keeps_its_id_as_its_key() {
        let target = Target::new(SessionKind::Codex, "t");
        assert_eq!(key(&scope(), &target, "a1", None), "a1");
    }

    #[test]
    fn instances_whose_fields_concatenate_to_the_same_bytes_get_different_keys() {
        let target = Target::new(SessionKind::Codex, "session");
        // "ab" + UUID and "a" + "b" + UUID: one plain concatenation.
        let shifted = Target::new(SessionKind::Codex, "sessiona");
        assert_ne!(
            key(&scope(), &target, "ab", Some(UUID)),
            key(&scope(), &shifted, "b", Some(UUID))
        );
        let site = Scope {
            site: "https://control.exampleexample-workspace".into(),
            workspace_id: String::new(),
            ..scope()
        };
        assert_ne!(
            key(&scope(), &target, "ab", Some(UUID)),
            key(&site, &target, "ab", Some(UUID))
        );
        // No person is not the same as an empty person.
        let nobody = Scope {
            user_id: None,
            ..scope()
        };
        let empty = Scope {
            user_id: Some(String::new()),
            ..scope()
        };
        assert_ne!(
            key(&nobody, &target, "ab", Some(UUID)),
            key(&empty, &target, "ab", Some(UUID))
        );
        // Every field counts.
        let base = key(&scope(), &target, "ab", Some(UUID));
        let other_uuid = "0b9f6a52-3c1e-4d7a-9e2f-5a6b7c8d9e00";
        assert_ne!(base, key(&scope(), &target, "ab", Some(other_uuid)));
        assert_ne!(base, key(&scope(), &target, "ac", Some(UUID)));
        let claude = Target::new(SessionKind::ClaudeTerminal, "session");
        assert_ne!(base, key(&scope(), &claude, "ab", Some(UUID)));
        assert_eq!(base, key(&scope(), &target, "ab", Some(UUID)));
    }

    #[test]
    fn only_a_uuid_or_null_is_an_incarnation() {
        assert_eq!(incarnation(&Value::Null), Ok(None));
        assert_eq!(
            incarnation(&json!(UUID.to_ascii_uppercase())),
            Ok(Some(UUID.into()))
        );
        for bad in [
            json!(""),
            json!("not-a-uuid"),
            json!(format!("{UUID}0")),
            json!("0b9f6a52:3c1e-4d7a-9e2f-5a6b7c8d9e0f"),
            json!(7),
            json!({"uuid": UUID}),
        ] {
            assert_eq!(incarnation(&bad), Err(()), "{bad}");
        }
    }
}
