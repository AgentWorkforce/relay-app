//! A Cursor chat's per-call proof: a compact JWS over
//! `{"v","kind":"cursor","sid","src","tuid","tool","args","iat","exp"}`, in
//! that order, signed with the installation's attestation key. Cloud
//! deep-equals `args` with the forwarded arguments minus the proof field.
use crate::claude_attestation::{Key, FIELD};
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};
use serde_json::Value;

/// The largest canonical `args` a proof carries.
pub(crate) const MAX_ARGS: usize = 32 << 10;
/// The largest proof Cloud accepts.
pub(crate) const MAX_PROOF: usize = 96 << 10;
/// How long a proof is valid after it is signed.
pub(crate) const LIFETIME_SECS: u64 = 60;

/// Cursor's `tool_use_id` as a proof's `tuid`: `^[A-Za-z0-9_.:-]{1,256}$`.
pub(crate) fn valid_tuid(tuid: &str) -> bool {
    (1..=256).contains(&tuid.len())
        && tuid
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b':' | b'-'))
}

/// `value` serialized with every object's keys sorted, whatever order the
/// map keeps them in.
pub(crate) struct Canonical<'a>(pub &'a Value);

impl Serialize for Canonical<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            Value::Object(map) => {
                let mut keys: Vec<_> = map.keys().collect();
                keys.sort();
                let mut out = serializer.serialize_map(Some(keys.len()))?;
                for key in keys {
                    out.serialize_entry(key, &Canonical(&map[key]))?;
                }
                out.end()
            }
            Value::Array(items) => {
                let mut out = serializer.serialize_seq(Some(items.len()))?;
                for item in items {
                    out.serialize_element(&Canonical(item))?;
                }
                out.end()
            }
            other => other.serialize(serializer),
        }
    }
}

/// `value` as canonical JSON: sorted keys, no whitespace.
pub(crate) fn canonical(value: &Value) -> String {
    serde_json::to_string(&Canonical(value)).unwrap_or_default()
}

/// A call's arguments as a proof signs them: the object without any proof
/// field a caller supplied, and its canonical form.
pub(crate) fn arguments(args: &Value) -> Result<(Value, String), &'static str> {
    let mut args = match args {
        Value::Object(map) => map.clone(),
        Value::Null => Default::default(),
        _ => return Err("cursor_call_invalid"),
    };
    args.remove(FIELD);
    let args = Value::Object(args);
    let text = canonical(&args);
    if text.len() > MAX_ARGS {
        return Err("cursor_args_too_large");
    }
    Ok((args, text))
}

#[derive(serde::Serialize)]
struct Claims<'a> {
    v: u8,
    kind: &'static str,
    sid: &'a str,
    src: &'a str,
    tuid: &'a str,
    tool: &'a str,
    args: Canonical<'a>,
    iat: u64,
    exp: u64,
}

/// The proof that chat `sid` of installation source `src` makes call `tuid`
/// of `tool` with `args`, signed at `now`.
pub(crate) fn sign_at(
    key: &Key,
    sid: &str,
    src: &str,
    tuid: &str,
    tool: &str,
    args: &Value,
    now: u64,
) -> Result<String, &'static str> {
    if !valid_tuid(tuid) {
        return Err("cursor_call_invalid");
    }
    let sid = sid.to_ascii_lowercase();
    let proof = key
        .sign_payload(&Claims {
            v: 1,
            kind: "cursor",
            sid: &sid,
            src,
            tuid,
            tool,
            args: Canonical(args),
            iat: now,
            exp: now + LIFETIME_SECS,
        })
        .ok_or("attestation_failed")?;
    if proof.len() > MAX_PROOF {
        return Err("cursor_args_too_large");
    }
    Ok(proof)
}

/// [`sign_at`] now.
pub(crate) fn sign(
    key: &Key,
    sid: &str,
    src: &str,
    tuid: &str,
    tool: &str,
    args: &Value,
) -> Result<String, &'static str> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "attestation_failed")?
        .as_secs();
    sign_at(key, sid, src, tuid, tool, args, now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use serde_json::json;

    fn part(proof: &str, at: usize) -> String {
        String::from_utf8(
            URL_SAFE_NO_PAD
                .decode(proof.split('.').nth(at).unwrap())
                .unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn the_payload_has_the_contract_members_in_order() {
        let home = tempfile::tempdir().unwrap();
        let key = Key::load_or_create(&home.path().join("key.json")).unwrap();
        let args = json!({"to":"peer","text":"hi","nested":{"b":1,"a":[{"z":0,"y":1}]}});
        let proof = sign_at(
            &key,
            "1069773C-0824-4FAD-B7F9-D3DC3C4ED148",
            "src-1",
            "toolu_01:a.b-c",
            "send_relay_message",
            &args,
            1_794_000_000,
        )
        .unwrap();
        assert_eq!(proof.split('.').count(), 3);
        let header: Value = serde_json::from_str(&part(&proof, 0)).unwrap();
        assert_eq!(header["alg"], "EdDSA");
        assert_eq!(header["kid"], key.kid());
        assert_eq!(header["typ"], "agent-relay-attest+jws");
        assert_eq!(
            part(&proof, 1),
            concat!(
                r#"{"v":1,"kind":"cursor","sid":"1069773c-0824-4fad-b7f9-d3dc3c4ed148","#,
                r#""src":"src-1","tuid":"toolu_01:a.b-c","tool":"send_relay_message","#,
                r#""args":{"nested":{"a":[{"y":1,"z":0}],"b":1},"text":"hi","to":"peer"},"#,
                r#""iat":1794000000,"exp":1794000060}"#
            )
        );
    }

    #[test]
    fn arguments_drop_the_proof_field_and_are_bounded() {
        let (args, text) =
            arguments(&json!({"b":"x y","a":1,"_agent_relay_attestation":"forged"})).unwrap();
        assert_eq!(args, json!({"a":1,"b":"x y"}));
        assert_eq!(text, r#"{"a":1,"b":"x y"}"#);
        assert_eq!(arguments(&Value::Null).unwrap().1, "{}");
        assert_eq!(arguments(&json!([1])), Err("cursor_call_invalid"));
        let big = json!({"text": "x".repeat(MAX_ARGS)});
        assert_eq!(arguments(&big), Err("cursor_args_too_large"));
        let fits = json!({"t": "x".repeat(MAX_ARGS - 8)});
        assert!(arguments(&fits).is_ok());
    }

    #[test]
    fn tool_use_ids_follow_the_contract() {
        for ok in ["a", "toolu_01ABC", "call:1.2-3_x", &"a".repeat(256)] {
            assert!(valid_tuid(ok), "{ok}");
        }
        for bad in ["", "a b", "a/b", "é", &"a".repeat(257), "a\n"] {
            assert!(!valid_tuid(bad), "{bad}");
        }
        let home = tempfile::tempdir().unwrap();
        let key = Key::load_or_create(&home.path().join("key.json")).unwrap();
        assert_eq!(
            sign_at(&key, "s", "src", "bad id", "t", &json!({}), 1),
            Err("cursor_call_invalid")
        );
    }
}
