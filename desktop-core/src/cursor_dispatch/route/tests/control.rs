//! A signed Cursor call checked the way Agent Relay Cloud's verifier
//! (`verifyClaudeAttestation` for `kind: "cursor"`) checks it, written from
//! its rules rather than from this crate's signer: it reads the body as sent,
//! with the key and source the identity sync registers.
use super::*;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use sha2::{Digest, Sha256};

const MAX_PROOF_BYTES: usize = 96 * 1024;
const MAX_LIFETIME: i64 = 60;
const MAX_FUTURE_SKEW: i64 = 30;

/// What Cloud holds for this installation: the sync's `source_id` and
/// `attestation_key`.
struct Registered {
    source_id: String,
    kid: String,
    public_key: String,
}

fn exact_keys(value: &Value, keys: &[&str]) -> bool {
    value
        .as_object()
        .is_some_and(|map| map.len() == keys.len() && keys.iter().all(|key| map.contains_key(*key)))
}

fn lower_uuid(text: &str) -> bool {
    let groups: Vec<&str> = text.split('-').collect();
    groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12])
        && groups.iter().all(|group| {
            group
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        })
}

fn charset(text: &str, max: usize, extra: &[u8]) -> bool {
    (1..=max).contains(&text.len())
        && text
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || extra.contains(&b))
}

/// `normalizeDesktopSourceId`: a UUID source compares case-insensitively.
fn normalized(source: &str) -> String {
    if lower_uuid(&source.to_ascii_lowercase()) {
        source.to_ascii_lowercase()
    } else {
        source.to_owned()
    }
}

fn safe_integer(value: &Value) -> Option<i64> {
    value.as_i64().filter(|n| n.unsigned_abs() < (1 << 53))
}

/// Cloud's verdict on `body`, the request bytes Cloud received, at `now`:
/// the verified chat, or the reason the call is refused.
fn verify(body: &[u8], registered: &Registered, now: i64) -> Result<String, &'static str> {
    let mut request: Value = serde_json::from_slice(body).map_err(|_| "malformed body")?;
    let proof = request["params"]["arguments"]
        .as_object_mut()
        .and_then(|arguments| arguments.remove(FIELD))
        .ok_or("no proof")?;
    let proof = proof.as_str().filter(|p| p.len() <= MAX_PROOF_BYTES);
    let parts: Vec<&str> = proof
        .ok_or("proof is not a compact JWS")?
        .split('.')
        .collect();
    let [head, payload, signature] = parts[..] else {
        return Err("proof is not a compact JWS");
    };
    let segment = |segment: &str| -> Result<Value, &'static str> {
        let bytes = URL_SAFE_NO_PAD
            .decode(segment)
            .map_err(|_| "malformed segment")?;
        let text = String::from_utf8(bytes).map_err(|_| "malformed segment")?;
        let value: Value = serde_json::from_str(&text).map_err(|_| "malformed segment")?;
        value
            .is_object()
            .then_some(value)
            .ok_or("malformed segment")
    };
    let header = segment(head)?;
    let kid = header["kid"].as_str().unwrap_or_default();
    if !exact_keys(&header, &["alg", "kid", "typ"])
        || header["alg"] != "EdDSA"
        || header["typ"] != "agent-relay-attest+jws"
        || kid.len() != 22
        || !charset(kid, 22, b"_-")
    {
        return Err("unexpected header");
    }
    let claims = segment(payload)?;
    if claims["kind"] != "cursor" {
        return Err("unexpected payload");
    }
    let names = [
        "v", "kind", "sid", "src", "tuid", "tool", "args", "iat", "exp",
    ];
    let text = |name: &str| claims[name].as_str().unwrap_or_default();
    let (iat, exp) = (safe_integer(&claims["iat"]), safe_integer(&claims["exp"]));
    if !exact_keys(&claims, &names)
        || claims["v"] != 1
        || !lower_uuid(text("sid"))
        || !claims["src"].is_string()
        || !(1..=200).contains(&text("src").chars().count())
        || !charset(text("tuid"), 256, b"_.:-")
        || !claims["args"].is_object()
        || !charset(text("tool"), 128, b"_.-")
        || iat.is_none()
        || exp.is_none()
    {
        return Err("unexpected payload");
    }
    if Some(text("tool")) != request["params"]["name"].as_str() {
        return Err("tool mismatch");
    }
    if claims["args"] != request["params"]["arguments"] {
        return Err("arguments mismatch");
    }
    let (iat, exp) = (iat.unwrap(), exp.unwrap());
    if exp < iat || exp - iat > MAX_LIFETIME {
        return Err("lifetime out of range");
    }
    if now > exp {
        return Err("expired");
    }
    if iat > now + MAX_FUTURE_SKEW {
        return Err("issued in the future");
    }
    let signature = URL_SAFE_NO_PAD
        .decode(signature)
        .map_err(|_| "malformed signature")?;
    let signature = Signature::from_slice(&signature).map_err(|_| "malformed signature")?;
    let raw = URL_SAFE_NO_PAD
        .decode(&registered.public_key)
        .ok()
        .filter(|raw| raw.len() == 32);
    let held =
        normalized(text("src")) == normalized(&registered.source_id) && kid == registered.kid;
    let raw = raw.filter(|raw| held && URL_SAFE_NO_PAD.encode(&Sha256::digest(raw)[..16]) == kid);
    let key = raw
        .and_then(|raw| VerifyingKey::from_bytes(raw.as_slice().try_into().ok()?).ok())
        .ok_or("unknown key")?;
    key.verify(format!("{head}.{payload}").as_bytes(), &signature)
        .map_err(|_| "bad signature")?;
    Ok(text("sid").to_owned())
}

/// The installation's registration, as the identity sync sends it.
fn held_by_cloud(f: &Fixture) -> Registered {
    let key = account(f)
        .attestation_key
        .get(&f.core.config.app_data_dir)
        .unwrap();
    let registration = key.registration();
    assert_eq!(registration["alg"], "Ed25519");
    Registered {
        source_id: f
            .core
            .store
            .read(|store| store.desktop_sync_source_id.clone())
            .unwrap(),
        kid: registration["kid"].as_str().unwrap().into(),
        public_key: registration["public_key"].as_str().unwrap().into(),
    }
}

/// Signs `arguments` for `tool` through the route and returns the bytes it
/// would post to Cloud.
fn signed(f: &Fixture, tool: &str, arguments: Value) -> Vec<u8> {
    announce(f, tool, &arguments);
    let (reply, posted) = run(dispatch(f, tool, arguments), &helper_table(f));
    assert!(reply.get("error").is_none(), "{reply}");
    serde_json::to_vec(&posted.unwrap().1).unwrap()
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

#[tokio::test]
async fn a_signed_identity_call_passes_cloud_s_verifier() {
    let f = fixture().await;
    let registered = held_by_cloud(&f);
    for (tool, arguments) in [
        (
            "send_relay_message",
            json!({"to":"synthetic-peer","text":"synthetic-message"}),
        ),
        (
            "send_relay_message",
            json!({"text":"é ✓ \"q\" \\ \n\u{1F600}","to":"p","z":[1, 2.5, -3, 1e21, null, true],
                   "a":{"y":{},"x":[]},"big":9007199254740993u64}),
        ),
        ("check_relay_inbox", json!({})),
        ("post_relay_channel", Value::Null),
    ] {
        let body = signed(&f, tool, arguments);
        assert_eq!(verify(&body, &registered, now()), Ok(sid()), "{tool}");
    }
}

#[tokio::test]
async fn the_verifier_refuses_what_cloud_refuses() {
    let f = fixture().await;
    let registered = held_by_cloud(&f);
    let arguments = json!({"to":"synthetic-peer","text":"synthetic-message"});
    let body = signed(&f, "send_relay_message", arguments);
    let request: Value = serde_json::from_slice(&body).unwrap();
    let changed = |change: &dyn Fn(&mut Value)| {
        let mut request = request.clone();
        change(&mut request);
        serde_json::to_vec(&request).unwrap()
    };
    let other_text = changed(&|r| r["params"]["arguments"]["text"] = json!("other"));
    assert_eq!(
        verify(&other_text, &registered, now()),
        Err("arguments mismatch")
    );
    let other_tool = changed(&|r| r["params"]["name"] = json!("post_relay_channel"));
    assert_eq!(
        verify(&other_tool, &registered, now()),
        Err("tool mismatch")
    );
    // The bounds come from the signed claims, not from a second clock read.
    let proof = request["params"]["arguments"][FIELD].as_str().unwrap();
    let claims: Value = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(proof.split('.').nth(1).unwrap())
            .unwrap(),
    )
    .unwrap();
    let (iat, exp) = (
        claims["iat"].as_i64().unwrap(),
        claims["exp"].as_i64().unwrap(),
    );
    assert_eq!(verify(&body, &registered, exp), Ok(sid()));
    assert_eq!(verify(&body, &registered, exp + 1), Err("expired"));
    assert_eq!(verify(&body, &registered, iat - MAX_FUTURE_SKEW), Ok(sid()));
    assert_eq!(
        verify(&body, &registered, iat - MAX_FUTURE_SKEW - 1),
        Err("issued in the future")
    );
    let elsewhere = Registered {
        source_id: "another-source".into(),
        ..held_by_cloud(&f)
    };
    assert_eq!(verify(&body, &elsewhere, now()), Err("unknown key"));
    let rotated = Registered {
        public_key: URL_SAFE_NO_PAD.encode([7u8; 32]),
        ..held_by_cloud(&f)
    };
    assert_eq!(verify(&body, &rotated, now()), Err("unknown key"));
    let forged = changed(&|r| {
        let proof = r["params"]["arguments"][FIELD].as_str().unwrap().to_owned();
        let parts: Vec<&str> = proof.split('.').collect();
        let mut claims: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).unwrap()).unwrap();
        claims["sid"] = json!("00000000-0000-4000-8000-000000000000");
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&claims).unwrap());
        r["params"]["arguments"][FIELD] = json!(format!("{}.{payload}.{}", parts[0], parts[2]));
    });
    assert_eq!(verify(&forged, &registered, now()), Err("bad signature"));
}
