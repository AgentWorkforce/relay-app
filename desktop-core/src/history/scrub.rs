//! The scrub seam: every session body desktop-core stores locally or uploads
//! passes through one [`ScrubEngine`] first, and reaches the disk or the
//! network only when the engine returns `Ok`.
//!
//! The engine is the `relayscrubber` crate, pinned to an exact version in
//! desktop-core/Cargo.toml and gtk/Cargo.toml. It sits behind
//! [`ScrubEngine::default_engine`] and nothing else: call sites hold a
//! `ScrubEngine` and never name the implementation behind it.
//!
//! The rule every call site keeps: a body the engine refuses is dropped and
//! recorded under the path's existing failure status. It is never stored raw,
//! never sent raw, and never retried with the raw bytes. Scrubbing a body
//! again before it is sent is allowed, so an engine must accept its own
//! output.
//!
//! An engine that panics is contained here, at the seam: the panic becomes a
//! [`ScrubError`] that [`ScrubError::panicked`] tells apart from a refusal,
//! and the panic's own message, which a slicing panic fills with the text it
//! was slicing, is never printed. Call sites quarantine the one record
//! rather than let one bad session take the whole collector down.
use std::cell::Cell;
use std::fmt;
use std::sync::{Arc, Once};

/// Where a scrubbed body is going next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrubSurface {
    /// A local store desktop-core writes: the delivery queue or the session
    /// store's remote evidence.
    LocalStore,
    /// A request body sent to a history destination.
    Upload,
}

/// Cheap context about the body being scrubbed. It never carries the body.
#[derive(Debug, Clone, Copy)]
pub struct ScrubContext<'a> {
    /// The provider or source kind (`claude`, `codex`, a connector id), when
    /// one body has a single source.
    pub source: Option<&'a str>,
    /// The session the body belongs to, when one body has a single session.
    pub session_id: Option<&'a str>,
    pub surface: ScrubSurface,
}

impl<'a> ScrubContext<'a> {
    pub fn new(surface: ScrubSurface) -> Self {
        Self {
            source: None,
            session_id: None,
            surface,
        }
    }

    pub fn source(mut self, source: &'a str) -> Self {
        self.source = Some(source);
        self
    }

    pub fn session(mut self, session_id: Option<&'a str>) -> Self {
        self.session_id = session_id;
        self
    }
}

/// Why a body could not be scrubbed. It carries the engine's reason only,
/// never any of the body's content, so it is safe to log and to record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrubError {
    reason: String,
    panicked: bool,
}

/// The status code a path records for a body its engine refused, where the
/// path has no failure kind of its own.
pub const SCRUB_FAILED: &str = "scrub_failed";

/// The reason a [`ScrubError`] carries when the engine panicked.
pub const ENGINE_PANICKED: &str = "engine_panicked";

impl ScrubError {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            panicked: false,
        }
    }

    /// The engine panicked on the body. A refusal is the engine's verdict on
    /// the body; a panic is the engine's own fault, which a later engine may
    /// not share, so call sites quarantine the body for that engine instead
    /// of treating it as refused.
    pub fn engine_panicked() -> Self {
        Self {
            reason: ENGINE_PANICKED.into(),
            panicked: true,
        }
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }

    /// Whether the engine panicked rather than refused.
    pub fn panicked(&self) -> bool {
        self.panicked
    }
}

impl fmt::Display for ScrubError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{SCRUB_FAILED}: {}", self.reason)
    }
}

impl std::error::Error for ScrubError {}

/// An implementation behind [`ScrubEngine`].
pub trait Scrub: Send + Sync {
    /// A short, stable name for diagnostics.
    fn name(&self) -> &str;
    /// `body` with whatever must not leave the machine removed, or why that
    /// cannot be done. The output may be the input unchanged.
    fn scrub(&self, body: &[u8], context: &ScrubContext<'_>) -> Result<Vec<u8>, ScrubError>;
}

/// The one scrub engine every session-body path holds. Cloning shares it.
#[derive(Clone)]
pub struct ScrubEngine(Arc<dyn Scrub>);

impl ScrubEngine {
    /// The engine production uses: the pinned `relayscrubber` crate.
    pub fn default_engine() -> Self {
        Self::new(RelayScrubber)
    }

    /// An engine over `scrub`.
    pub fn new(scrub: impl Scrub + 'static) -> Self {
        Self(Arc::new(scrub))
    }

    pub fn name(&self) -> &str {
        self.0.name()
    }

    /// `body` through the engine. A panic inside the engine is caught and
    /// returned as [`ScrubError::engine_panicked`] with its message withheld
    /// (see [`contained`]), so no engine bug unwinds into the caller.
    pub fn scrub(&self, body: &[u8], context: &ScrubContext<'_>) -> Result<Vec<u8>, ScrubError> {
        contained(|| self.0.scrub(body, context))
    }

    /// [`Self::scrub`] for a UTF-8 body: output that is not UTF-8 is a
    /// failure, never a lossy conversion.
    pub fn scrub_text(&self, body: &str, context: &ScrubContext<'_>) -> Result<String, ScrubError> {
        String::from_utf8(self.scrub(body.as_bytes(), context)?)
            .map_err(|_| ScrubError::new("engine output is not UTF-8"))
    }

    /// `value` through the engine as compact JSON, parsed back as `T`.
    /// Output that no longer parses as `T` is a failure.
    pub fn scrub_json<T>(&self, value: &T, context: &ScrubContext<'_>) -> Result<T, ScrubError>
    where
        T: serde::Serialize + serde::de::DeserializeOwned,
    {
        let body =
            serde_json::to_vec(value).map_err(|_| ScrubError::new("body does not serialize"))?;
        let scrubbed = self.scrub(&body, context)?;
        serde_json::from_slice(&scrubbed)
            .map_err(|_| ScrubError::new("engine output is not the body's JSON shape"))
    }
}

impl Default for ScrubEngine {
    fn default() -> Self {
        Self::default_engine()
    }
}

impl fmt::Debug for ScrubEngine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ScrubEngine").field(&self.name()).finish()
    }
}

thread_local! {
    /// Set while this thread runs an engine, for [`withhold_engine_panics`].
    static SCRUBBING: Cell<bool> = const { Cell::new(false) };
}

/// Runs `scrub` with a panic in it caught and turned into
/// [`ScrubError::engine_panicked`]. The engine is pure over its input, so
/// nothing it could leave half-done outlives the unwind.
fn contained(scrub: impl FnOnce() -> Result<Vec<u8>, ScrubError>) -> Result<Vec<u8>, ScrubError> {
    withhold_engine_panics();
    let outer = SCRUBBING.with(|flag| flag.replace(true));
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(scrub));
    SCRUBBING.with(|flag| flag.set(outer));
    result.unwrap_or_else(|_| Err(ScrubError::engine_panicked()))
}

/// Installs, once per process, a panic hook in front of the one already set:
/// a panic on a thread inside [`contained`] prints one fixed line with the
/// panic's location and never its message, because a string-slicing panic
/// quotes the session text it was slicing. Every other panic reaches the
/// previous hook unchanged.
fn withhold_engine_panics() {
    static INSTALL: Once = Once::new();
    INSTALL.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if SCRUBBING.with(Cell::get) {
                let location = info
                    .location()
                    .map(|at| format!("{}:{}", at.file(), at.line()))
                    .unwrap_or_default();
                eprintln!(
                    "scrub engine panicked reason={ENGINE_PANICKED} location={location} (message withheld)"
                );
            } else {
                previous(info);
            }
        }));
    });
}

/// The name [`ScrubEngine::default_engine`] reports, and the engine name
/// stored with what it scrubbed. It carries the exact crate version: a held
/// copy is scrubbed again on release only when the engine that scrubbed it has
/// a different name, so every version bump must change this name too. A test
/// checks it against the pin in desktop-core/Cargo.toml and gtk/Cargo.toml.
pub const DEFAULT_ENGINE: &str = "relayscrubber-0.5.0-alpha.10";

/// The `relayscrubber` crate's Secrets policy.
///
/// A body that parses as JSON is scrubbed structurally, value by value, and
/// re-serialized compactly, so the output is still JSON of the same shape. A
/// multi-line body whose every nonblank line is JSON (NDJSON) is scrubbed the
/// same way, record by record. Any other UTF-8 body is scrubbed as text. A body that is not UTF-8, or that
/// the crate refuses (over its size limits), is a failure: the caller drops
/// it and never sends or stores the raw bytes. Failures carry the crate's
/// stable reason code only, never body content.
struct RelayScrubber;

impl Scrub for RelayScrubber {
    fn name(&self) -> &str {
        DEFAULT_ENGINE
    }

    fn scrub(&self, body: &[u8], _context: &ScrubContext<'_>) -> Result<Vec<u8>, ScrubError> {
        let text = std::str::from_utf8(body).map_err(|_| ScrubError::new("body is not UTF-8"))?;
        match relayscrubber::scrub_json_text_checked(text) {
            Ok((value, _)) => {
                return serde_json::to_vec(&value)
                    .map_err(|_| ScrubError::new("scrubbed JSON does not serialize"));
            }
            Err(error) if error.code != INVALID_JSON => return Err(ScrubError::new(error.code)),
            Err(_) => {}
        }
        if text.contains('\n') {
            match relayscrubber::scrub_records(body) {
                Ok(output) if output.record_count > 0 => return Ok(output.bytes),
                Ok(_) => {}
                Err(error) if error.code != INVALID_JSON => {
                    return Err(ScrubError::new(error.code));
                }
                Err(_) => {}
            }
        }
        relayscrubber::scrub_text_checked(text)
            .map(String::into_bytes)
            .map_err(|error| ScrubError::new(error.code))
    }
}

/// The crate's refusal code for input that is not JSON: the body is then
/// scrubbed as text instead. Every other refusal is a failure.
const INVALID_JSON: &str = "invalid_json";

/// Engines the seam's tests inject.
#[cfg(test)]
pub(crate) mod testing {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Replaces every `from` with `to`, counting its calls.
    pub struct Rewrite {
        pub from: &'static str,
        pub to: &'static str,
        pub calls: Arc<AtomicUsize>,
    }

    impl Scrub for Rewrite {
        fn name(&self) -> &str {
            "test-rewrite"
        }

        fn scrub(&self, body: &[u8], _context: &ScrubContext<'_>) -> Result<Vec<u8>, ScrubError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let text = std::str::from_utf8(body).map_err(|_| ScrubError::new("not UTF-8"))?;
            Ok(text.replace(self.from, self.to).into_bytes())
        }
    }

    pub fn rewrite(from: &'static str, to: &'static str) -> ScrubEngine {
        ScrubEngine::new(Rewrite {
            from,
            to,
            calls: Arc::default(),
        })
    }

    /// Refuses every body, counting its calls.
    pub struct Refuse(pub Arc<AtomicUsize>);

    impl Scrub for Refuse {
        fn name(&self) -> &str {
            "test-refuse"
        }

        fn scrub(&self, _body: &[u8], _context: &ScrubContext<'_>) -> Result<Vec<u8>, ScrubError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Err(ScrubError::new("test engine refuses"))
        }
    }

    pub fn refuse() -> (ScrubEngine, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (ScrubEngine::new(Refuse(calls.clone())), calls)
    }

    /// Panics on every body that contains `trigger`, slicing a byte range
    /// that starts one past its end as relayscrubber 0.5.0-alpha.10 did, and
    /// scrubs any other body by replacing `from` with `to`. Named `name`, so
    /// a test can stand in a later engine version.
    pub struct PanicOn {
        pub name: &'static str,
        pub trigger: &'static str,
        pub from: &'static str,
        pub to: &'static str,
    }

    impl Scrub for PanicOn {
        fn name(&self) -> &str {
            self.name
        }

        fn scrub(&self, body: &[u8], _context: &ScrubContext<'_>) -> Result<Vec<u8>, ScrubError> {
            let text = std::str::from_utf8(body).map_err(|_| ScrubError::new("not UTF-8"))?;
            if text.contains(self.trigger) {
                let end = std::hint::black_box(text.len() - 1);
                let start = end + 1;
                let _ = &text[start..end];
                // A message that quotes the body, as a slicing panic can.
                panic!("engine failed on {text}");
            }
            Ok(text.replace(self.from, self.to).into_bytes())
        }
    }

    pub fn panic_on(
        name: &'static str,
        trigger: &'static str,
        from: &'static str,
        to: &'static str,
    ) -> ScrubEngine {
        ScrubEngine::new(PanicOn {
            name,
            trigger,
            from,
            to,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    #[test]
    fn the_default_engine_is_the_pinned_relayscrubber() {
        let engine = ScrubEngine::default_engine();
        assert_eq!(engine.name(), DEFAULT_ENGINE);
        assert_eq!(ScrubEngine::default().name(), DEFAULT_ENGINE);
        assert!(DEFAULT_ENGINE.starts_with("relayscrubber-"));
        let context = ScrubContext::new(ScrubSurface::Upload);
        let body = b"{\"text\":\"unchanged\"}";
        assert_eq!(engine.scrub(body, &context).unwrap(), body.to_vec());
    }

    #[test]
    fn the_engine_name_carries_the_exact_pinned_version() {
        let version = DEFAULT_ENGINE.strip_prefix("relayscrubber-").unwrap();
        let pin = format!("relayscrubber = \"={version}\"");
        for manifest in [
            include_str!("../../Cargo.toml"),
            include_str!("../../../gtk/Cargo.toml"),
        ] {
            assert!(manifest.contains(&pin), "{pin} is missing from a manifest");
        }
    }

    // Synthetic values only; none of these is a real credential.
    const FAKE_KEY: &str = "sk-proj-ABCDEFGHIJKLMNOPQRSTUVWX1234567890abcd";

    #[test]
    fn the_default_engine_redacts_a_secret_in_json_and_keeps_the_shape() {
        let engine = ScrubEngine::default_engine();
        let context = ScrubContext::new(ScrubSurface::Upload);
        let value = serde_json::json!({
            "text": format!("export OPENAI_API_KEY={FAKE_KEY}"),
            "session_id": 57243,
        });
        let scrubbed = engine.scrub_json(&value, &context).unwrap();
        let text = scrubbed["text"].as_str().unwrap();
        assert!(!text.contains(FAKE_KEY), "{text}");
        assert!(text.contains("[REDACTED]"), "{text}");
        assert_eq!(scrubbed["session_id"], 57243);
    }

    #[test]
    fn the_default_engine_redacts_a_secret_in_plain_text() {
        let engine = ScrubEngine::default_engine();
        let context = ScrubContext::new(ScrubSurface::LocalStore);
        let body = format!("curl -H 'Authorization: Bearer {FAKE_KEY}' https://example.invalid");
        let scrubbed = engine.scrub_text(&body, &context).unwrap();
        assert!(!scrubbed.contains(FAKE_KEY), "{scrubbed}");
        assert!(scrubbed.contains("[REDACTED]"), "{scrubbed}");
    }

    #[test]
    fn the_default_engine_accepts_its_own_output() {
        let engine = ScrubEngine::default_engine();
        let context = ScrubContext::new(ScrubSurface::Upload);
        for body in [
            serde_json::json!({"text": format!("token={FAKE_KEY}")}).to_string(),
            format!("password: {FAKE_KEY}\nplain words stay"),
        ] {
            let once = engine.scrub(body.as_bytes(), &context).unwrap();
            let twice = engine.scrub(&once, &context).unwrap();
            assert_eq!(once, twice);
        }
    }

    // Since alpha.9 the engine keeps an exact `project_key` whose value is a
    // repository identity (ai-hist's `Remote` method, `host.tld/owner/repo`),
    // so History can tag a session with its project. A path fallback, a
    // vendor-style key, a remote on an IP address or on a host with no domain
    // and a prefixed name stay redacted, as does a credential beside the key.
    #[test]
    fn the_default_engine_keeps_a_repository_project_key() {
        const IDENTITY: &str = "github.com/example-org/example-repo";
        let engine = ScrubEngine::default_engine();
        let context = ScrubContext::new(ScrubSurface::Upload);
        let record = serde_json::json!({
            "project_key": IDENTITY,
            "project_key_method": "remote",
            "api_key": FAKE_KEY,
        });
        let scrubbed = engine.scrub_json(&record, &context).unwrap();
        assert_eq!(scrubbed["project_key"], IDENTITY);
        assert_eq!(scrubbed["project_key_method"], "remote");
        assert_eq!(scrubbed["api_key"], "[REDACTED]");

        let body = format!(
            "{record}\n{}\n",
            serde_json::json!({"note": "ordinary words"})
        );
        let scrubbed = String::from_utf8(engine.scrub(body.as_bytes(), &context).unwrap()).unwrap();
        assert!(scrubbed.contains(IDENTITY), "{scrubbed}");
        assert!(!scrubbed.contains(FAKE_KEY), "{scrubbed}");

        for value in [
            "/Users/someone/Projects/app",
            "c0ab12cd_SYNTHaZ91bY82cX73dW64eV55",
            "10.0.0.5/example-org/example-repo",
            "gitserver/example-org/example-repo",
        ] {
            let scrubbed = engine
                .scrub_json(&serde_json::json!({"project_key": value}), &context)
                .unwrap();
            assert_eq!(scrubbed["project_key"], "[REDACTED]", "{value}");
        }
        let scrubbed = engine
            .scrub_json(&serde_json::json!({"deta_project_key": IDENTITY}), &context)
            .unwrap();
        assert_eq!(scrubbed["deta_project_key"], "[REDACTED]");
    }

    // alpha.10 redacts a password told in a sentence, however short the
    // value, in a JSON string as in plain text. A sentence about passwords
    // that discloses nothing is unchanged.
    #[test]
    fn the_default_engine_redacts_a_password_told_in_a_sentence() {
        let engine = ScrubEngine::default_engine();
        let context = ScrubContext::new(ScrubSurface::Upload);
        for (told, scrubbed) in [
            ("my password is x", "my password is [REDACTED]"),
            ("the password is #", "the password is [REDACTED]"),
            ("my password is 'hunter2'", "my password is '[REDACTED]'"),
            (
                "my passphrase is velvet river lantern",
                "my passphrase is [REDACTED]",
            ),
            ("my pwd is x", "my pwd is [REDACTED]"),
            ("the passcode was 4711", "the passcode was [REDACTED]"),
            ("the password is required", "the password is required"),
            (
                "reset your password in Settings",
                "reset your password in Settings",
            ),
        ] {
            assert_eq!(engine.scrub_text(told, &context).unwrap(), scrubbed);
            let record = engine
                .scrub_json(&serde_json::json!({"text": told}), &context)
                .unwrap();
            assert_eq!(record["text"], scrubbed, "{told}");
        }
    }

    #[test]
    fn the_default_engine_keeps_ndjson_records_valid() {
        let engine = ScrubEngine::default_engine();
        let context = ScrubContext::new(ScrubSurface::Upload);
        let body = format!(
            "{}\n{}\n",
            serde_json::json!({"text": format!("key={FAKE_KEY}")}),
            serde_json::json!({"note": "ordinary words", "token": FAKE_KEY}),
        );
        let scrubbed = String::from_utf8(engine.scrub(body.as_bytes(), &context).unwrap()).unwrap();
        assert!(!scrubbed.contains(FAKE_KEY), "{scrubbed}");
        let records: Vec<serde_json::Value> = scrubbed
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(records.len(), 2);
        assert_eq!(records[1]["note"], "ordinary words");
    }

    #[test]
    fn the_default_engine_refuses_non_utf8_without_echoing_it() {
        let engine = ScrubEngine::default_engine();
        let context = ScrubContext::new(ScrubSurface::Upload);
        let mut body = b"recognisable-non-utf8-body".to_vec();
        body.push(0xff);
        let error = engine.scrub(&body, &context).unwrap_err();
        assert_eq!(error.reason(), "body is not UTF-8");
        assert!(!error.to_string().contains("recognisable-non-utf8-body"));
    }

    #[test]
    fn the_default_engine_reports_a_size_refusal_code_without_body_content() {
        let engine = ScrubEngine::default_engine();
        let context = ScrubContext::new(ScrubSurface::Upload);
        let marker = "recognisable-oversized-json-content";
        let body = format!(
            "{{\"text\":\"{marker}{}\"}}",
            "x".repeat(relayscrubber::MAX_RECORD_BYTES)
        );
        let error = engine.scrub(body.as_bytes(), &context).unwrap_err();
        assert_eq!(error.reason(), "record_too_large");
        assert!(!error.to_string().contains(marker));
    }

    #[test]
    fn json_output_that_lost_its_shape_is_a_failure() {
        let engine = rewrite("{", "");
        let context = ScrubContext::new(ScrubSurface::LocalStore);
        let error = engine
            .scrub_json(&serde_json::json!({"text": "secret"}), &context)
            .unwrap_err();
        assert!(error.to_string().starts_with(SCRUB_FAILED));
        assert!(!error.to_string().contains("secret"));
    }

    #[test]
    fn an_engine_panic_is_contained_as_a_panicked_failure_without_its_content() {
        let engine = panic_on("test-panic-on", "poison", "a", "a");
        let context = ScrubContext::new(ScrubSurface::LocalStore);
        let error = engine
            .scrub_json(&serde_json::json!({"text": "poison here"}), &context)
            .unwrap_err();
        assert!(error.panicked());
        assert_eq!(error.reason(), ENGINE_PANICKED);
        assert!(!error.to_string().contains("poison"));
        // The engine still serves the next body, and a refusal is no panic.
        assert_eq!(engine.scrub_text("fine", &context).unwrap(), "fine");
        let (refusing, _) = refuse();
        assert!(!refusing.scrub_text("x", &context).unwrap_err().panicked());
    }

    #[test]
    fn non_utf8_output_is_a_failure_not_a_lossy_string() {
        struct Binary;
        impl Scrub for Binary {
            fn name(&self) -> &str {
                "binary"
            }
            fn scrub(&self, _: &[u8], _: &ScrubContext<'_>) -> Result<Vec<u8>, ScrubError> {
                Ok(vec![0xff, 0xfe])
            }
        }
        let engine = ScrubEngine::new(Binary);
        let context = ScrubContext::new(ScrubSurface::Upload);
        assert!(engine.scrub_text("body", &context).is_err());
    }
}
