//! A site's analytics gateway (`POST <site>/api/v1/analytics/events`), the
//! on-site path of `posthog`. It takes only two registered kinds, so a site
//! build sends nothing else: `desktop.app_started` (`desktop_version`, `os`)
//! and `desktop.sign_in_completed` (no properties). Any other kind, field or
//! property, or a reserved name at any depth, refuses the whole request.
//!
//! The gateway needs the signed-in session, so `app_started` is queued at the
//! first attach with an account in each process, not at launch (and again at
//! a later attach while no request carrying it has been taken), and
//! `sign_in_completed` when a sign-in or renewal finishes. Events before
//! sign-in are dropped, never kept for later.
//!
//! Up to [`MAX_EVENTS`] events go in one `client-batch/v1` request. `202` is
//! acceptance (`queued: false` included); any other answer is final, except
//! `429`, `5xx` and an unreachable site, which are tried [`MAX_ATTEMPTS`]
//! times with a 1 s, then 2 s wait (`Retry-After` delta-seconds instead, at
//! most 30 s). A retry stops at an opt-out or shutdown.

use super::{json, platform, Shared, TelemetrySender, Value};
use super::{
    posthog::{Transport, TransportFailure},
    Duration,
};

/// The gateway's path below the site's origin; nothing is appended.
pub const PATH: &str = "/api/v1/analytics/events";
const SCHEMA: &str = "agent-relay.analytics.client-batch/v1";
/// Events per request (`GATEWAY_MAX_EVENTS`).
pub const MAX_EVENTS: usize = 20;
/// Request body limit (`GATEWAY_MAX_BODY_BYTES`).
const MAX_BODY_BYTES: usize = 32_768;
/// Tries per batch, the first included.
pub const MAX_ATTEMPTS: u32 = 3;
const BACKOFF: Duration = Duration::from_secs(1);
const RETRY_CAP: Duration = Duration::from_secs(30);

/// What became of one batch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Delivery {
    Sent,
    /// Final: the batch is dropped.
    Dropped(String),
    /// Worth another try, after `after` when the receiver named a wait.
    Retry {
        reason: String,
        after: Option<Duration>,
    },
}

impl Delivery {
    pub fn into_result(self) -> Result<(), String> {
        match self {
            Delivery::Sent => Ok(()),
            Delivery::Dropped(reason) | Delivery::Retry { reason, .. } => Err(reason),
        }
    }
}

/// `desktop.app_started`, or `None` when `version` is outside the gateway's
/// version grammar, which would refuse the request.
pub(super) fn app_started(version: &str, occurred_at: String) -> Option<Value> {
    valid_version(version).then(|| {
        event(
            "desktop.app_started",
            occurred_at,
            json!({"desktop_version": version, "os": platform()}),
        )
    })
}

/// `desktop.sign_in_completed`.
pub(super) fn sign_in_completed(occurred_at: String) -> Value {
    event("desktop.sign_in_completed", occurred_at, json!({}))
}

fn event(kind: &str, occurred_at: String, properties: Value) -> Value {
    json!({
        "event": kind,
        "occurred_at": occurred_at,
        "client_event_id": super::new_uuid(),
        "properties": properties,
    })
}

/// `MAJOR.MINOR.PATCH` (1 to 5 digits each), then optionally `-prerelease`
/// and `+build` of dot-separated `[0-9A-Za-z-]+` identifiers; 32 characters
/// at most.
pub(super) fn valid_version(text: &str) -> bool {
    let identifiers = |part: &str| {
        part.split('.').all(|identifier| {
            !identifier.is_empty()
                && identifier
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    };
    let (rest, build) = match text.split_once('+') {
        Some((rest, build)) => (rest, Some(build)),
        None => (text, None),
    };
    let (core, prerelease) = match rest.split_once('-') {
        Some((core, prerelease)) => (core, Some(prerelease)),
        None => (rest, None),
    };
    let numbers: Vec<&str> = core.split('.').collect();
    text.len() <= 32
        && numbers.len() == 3
        && numbers.iter().all(|number| {
            (1..=5).contains(&number.len()) && number.bytes().all(|byte| byte.is_ascii_digit())
        })
        && prerelease.is_none_or(identifiers)
        && build.is_none_or(identifiers)
}

/// Whether `batch` carries `desktop.app_started`.
fn carries_app_started(batch: &[Value]) -> bool {
    batch
        .iter()
        .any(|event| event["event"] == "desktop.app_started")
}

/// One gateway request for `batch` with the session's `bearer`.
pub(super) fn post(
    transport: &dyn Transport,
    url: &str,
    bearer: &str,
    batch: Vec<Value>,
) -> Delivery {
    if batch.is_empty() {
        return Delivery::Sent;
    }
    let body = json!({"schema": SCHEMA, "client": "desktop", "events": batch});
    if body.to_string().len() > MAX_BODY_BYTES {
        return Delivery::Dropped("The analytics batch is too large.".into());
    }
    match transport.checked_post(url, Some(bearer), body) {
        Err(TransportFailure::Permanent(reason)) => Delivery::Dropped(reason),
        Err(TransportFailure::Transient(reason)) => Delivery::Retry {
            reason,
            after: None,
        },
        Ok(answer) => match answer.status {
            202 => Delivery::Sent,
            429 => Delivery::Retry {
                reason: "The site's analytics gateway is busy.".into(),
                after: answer.retry_after.as_deref().and_then(retry_after),
            },
            500..=599 => Delivery::Retry {
                reason: format!("The site's analytics gateway answered {}.", answer.status),
                after: None,
            },
            status => Delivery::Dropped(format!("The site's analytics gateway answered {status}.")),
        },
    }
}

/// `Retry-After` as unsigned decimal delta-seconds, at most 30 s; anything
/// else (an HTTP date included) leaves the normal backoff.
fn retry_after(text: &str) -> Option<Duration> {
    let text = text.trim();
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let seconds = text.parse::<u64>().unwrap_or(u64::MAX);
    Some(Duration::from_secs(seconds).min(RETRY_CAP))
}

/// Sends `batch`, trying again as [`Delivery::Retry`] asks, up to
/// [`MAX_ATTEMPTS`]. True when it was taken. A `desktop.app_started` that was
/// not taken is queued again at the next attach.
pub(super) fn deliver(shared: &Shared, sender: &dyn TelemetrySender, batch: Vec<Value>) -> bool {
    let starts = carries_app_started(&batch);
    let sent = try_deliver(shared, sender, batch);
    if starts && !sent {
        shared.lock().app_started = false;
    }
    sent
}

fn try_deliver(shared: &Shared, sender: &dyn TelemetrySender, batch: Vec<Value>) -> bool {
    let mut attempt = 1;
    loop {
        let wait = match sender.deliver(batch.clone()) {
            Delivery::Sent => return true,
            Delivery::Retry { after, .. } if attempt < MAX_ATTEMPTS => {
                after.unwrap_or(BACKOFF * 2u32.pow(attempt - 1))
            }
            Delivery::Retry { .. } | Delivery::Dropped(_) => return false,
        };
        let state = shared.lock();
        let (state, _) = shared
            .changed
            .wait_timeout_while(state, wait.min(RETRY_CAP), |state| {
                !state.shutdown && state.enabled
            })
            .unwrap_or_else(|poison| poison.into_inner());
        if state.shutdown || !state.enabled {
            return false;
        }
        attempt += 1;
    }
}

/// `first` and, unless it goes alone, the queued events after it that may
/// share its request, up to `limit` in all. A `$identify` goes alone.
pub(super) fn batch(
    state: &mut super::State,
    first: Value,
    shareable: bool,
    limit: usize,
) -> Vec<Value> {
    let mut batch = vec![first];
    while shareable
        && batch.len() < limit
        && state
            .queue
            .front()
            .is_some_and(|next| next.merges.is_none())
    {
        batch.extend(state.queue.pop_front().map(|next| next.event));
    }
    batch
}

impl super::Telemetry {
    /// Whether events go to a site's analytics gateway.
    pub(super) fn on_site(&self) -> bool {
        self.config
            .project
            .as_ref()
            .is_some_and(super::PostHogProject::on_site)
    }

    /// Queues `desktop.app_started` once per process, at the first attach
    /// with an account: the gateway drops anything sent before sign-in. A
    /// start that was not taken (no fresh session yet, say) is queued again
    /// at the next attach.
    pub(super) fn app_started(&self, state: &mut super::State) -> bool {
        if state.app_started || !self.may_queue(state) {
            return false;
        }
        state.app_started = true;
        let event = app_started(&self.config.version, self.timestamp());
        event.is_some_and(|event| queue(state, event))
    }

    /// A sign-in (or its renewal) finished and attached an account. A site's
    /// gateway gets `desktop.sign_in_completed`; nothing else records it.
    pub fn sign_in_completed(&self) {
        let mut state = self.shared.lock();
        if !self.on_site() || !self.may_queue(&state) {
            return;
        }
        if queue(&mut state, sign_in_completed(self.timestamp())) {
            drop(state);
            self.shared.changed.notify_all();
        }
    }

    fn may_queue(&self, state: &super::State) -> bool {
        !state.shutdown && state.enabled && state.account.is_some()
    }
}

fn queue(state: &mut super::State, event: Value) -> bool {
    let room = state.queue.len() < super::QUEUE_LIMIT;
    if room {
        state.queue.push_back(super::Queued {
            event,
            merges: None,
        });
    }
    room
}

#[cfg(test)]
mod tests {
    use super::super::posthog::tests::{Recorded, Signed};
    use super::super::*;
    use super::*;
    use std::sync::mpsc;

    const SITE: &str = "https://site.example";
    const URL: &str = "https://site.example/api/v1/analytics/events";
    const RESERVED: [&str; 7] = [
        "deployment_id",
        "component",
        "tenant_id",
        "workspace_id",
        "event_id",
        "trace_id",
        "version",
    ];

    struct Site {
        _dir: tempfile::TempDir,
        telemetry: Arc<Telemetry>,
        recorded: Recorded,
        signed: Arc<Signed>,
    }

    /// A site build's pipeline over `transport`, which records into
    /// `recorded`, with nobody signed in yet.
    fn site_with(version: &str, recorded: Recorded, transport: impl Transport + 'static) -> Site {
        let dir = tempfile::tempdir().unwrap();
        let config = TelemetryConfig {
            app_data_dir: dir.path().join("data"),
            home: dir.path().to_path_buf(),
            env: BTreeMap::new(),
            version: version.into(),
            project: PostHogProject::site_collector(SITE),
            session: SessionSlot::default(),
            clock: Arc::new(|| SystemTime::UNIX_EPOCH + Duration::from_secs(1_794_139_200)),
            ticks: Arc::new(Instant::now),
        };
        let signed = Arc::new(Signed(Mutex::new(None)));
        let weak: std::sync::Weak<dyn SessionBearer> = Arc::downgrade(&signed) as _;
        assert!(config.session.set(weak).is_ok());
        let sender = PostHogSender::with_transport(
            config.project.clone().unwrap(),
            config.session.clone(),
            transport,
        );
        Site {
            _dir: dir,
            telemetry: Telemetry::new(config, Arc::new(sender)),
            recorded,
            signed,
        }
    }

    fn site(version: &str) -> Site {
        let recorded = Recorded::default();
        site_with(version, recorded.clone(), recorded)
    }

    impl Site {
        fn sign_in(&self) {
            *self.signed.0.lock().unwrap() = Some("session-token");
            self.telemetry.attach(Some(account()));
        }

        fn requests(&self) -> Vec<(String, Option<String>, Value)> {
            assert!(self.telemetry.flush(Duration::from_secs(10)));
            self.recorded.0.lock().unwrap().clone()
        }

        fn events(&self) -> Vec<Value> {
            self.requests()
                .into_iter()
                .flat_map(|(_, _, body)| body["events"].as_array().unwrap().clone())
                .collect()
        }
    }

    fn account() -> TelemetryAccount {
        TelemetryAccount {
            user_id: "user_123".into(),
            organization_id: Some("org_456".into()),
            workspace_id: Some("ws_789".into()),
        }
    }

    fn operation() -> TelemetryEvent {
        TelemetryEvent::new(TelemetryOperation::SignIn, TelemetryOutcome::Failed)
            .failure(TelemetryFailure::Unexpected)
    }

    fn keys(value: &Value) -> Vec<&str> {
        let mut keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        keys.sort_unstable();
        keys
    }

    /// Every object key at any depth.
    fn all_keys(value: &Value, found: &mut Vec<String>) {
        match value {
            Value::Object(map) => map.iter().for_each(|(key, value)| {
                found.push(key.clone());
                all_keys(value, found);
            }),
            Value::Array(items) => items.iter().for_each(|item| all_keys(item, found)),
            _ => {}
        }
    }

    #[test]
    fn a_site_build_sends_only_app_started_and_sign_in_completed_to_the_gateway() {
        let site = site("2026.11.4-beta.7");
        // Before sign-in: nothing, and nothing kept for later.
        site.telemetry.record(operation());
        site.telemetry.sign_in_completed();
        site.telemetry.attach(None);
        assert!(site.requests().is_empty());

        site.sign_in();
        site.telemetry.record(operation());
        site.telemetry.sign_in_completed();
        // A second attach in the same process is no second start.
        site.telemetry.attach(Some(account()));
        site.telemetry.record(operation());
        let requests = site.requests();
        assert!(!requests.is_empty());
        for (url, bearer, body) in &requests {
            assert_eq!(url, URL);
            assert_eq!(bearer.as_deref(), Some("session-token"));
            assert_eq!(keys(body), ["client", "events", "schema"]);
            assert_eq!(body["schema"], "agent-relay.analytics.client-batch/v1");
            assert_eq!(body["client"], "desktop");
            assert!(!body.to_string().contains("session-token"));
            assert!(!body.to_string().contains("$identify"));
        }
        let events = site.events();
        let kinds: Vec<&str> = events
            .iter()
            .map(|e| e["event"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["desktop.app_started", "desktop.sign_in_completed"]);
        for event in &events {
            assert_eq!(
                keys(event),
                ["client_event_id", "event", "occurred_at", "properties"]
            );
            let id = event["client_event_id"].as_str().unwrap();
            assert_eq!(parse_uuid(id).as_deref(), Some(id));
            assert_eq!(id.as_bytes()[14], b'4');
            assert_eq!(event["occurred_at"], "2026-11-08T12:00:00Z");
        }
        assert_ne!(events[0]["client_event_id"], events[1]["client_event_id"]);
        assert_eq!(
            events[0]["properties"],
            json!({"desktop_version": "2026.11.4-beta.7", "os": platform()})
        );
        assert!(["macos", "windows", "linux"].contains(&platform()));
        assert_eq!(events[1]["properties"], json!({}));

        // Signed out: nothing more leaves.
        *site.signed.0.lock().unwrap() = None;
        site.telemetry.sign_in_completed();
        assert_eq!(site.requests().len(), requests.len());
    }

    #[test]
    fn an_on_site_body_carries_no_reserved_field_at_any_depth() {
        let site = site("2026.11.4+build.7");
        site.sign_in();
        site.telemetry.sign_in_completed();
        let requests = site.requests();
        assert!(!requests.is_empty());
        for (_, _, body) in &requests {
            let mut found = Vec::new();
            all_keys(body, &mut found);
            for name in RESERVED {
                assert!(!found.iter().any(|key| key == name), "{name} in {body}");
            }
            for name in ["api_key", "distinct_id", "$set", "operation", "os_version"] {
                assert!(!found.iter().any(|key| key == name), "{name} in {body}");
            }
        }
    }

    #[test]
    fn on_site_operation_events_and_identify_never_make_a_request() {
        let site = site("2026.11.4");
        site.telemetry.record(operation());
        *site.signed.0.lock().unwrap() = Some("session-token");
        site.telemetry.attach(Some(account()));
        for _ in 0..5 {
            site.telemetry.record(operation());
        }
        let events = site.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["event"], "desktop.app_started");
    }

    #[test]
    fn a_version_outside_the_grammar_drops_app_started() {
        let site = site("dev build");
        site.sign_in();
        assert!(site.requests().is_empty());
        site.telemetry.sign_in_completed();
        let kinds: Vec<Value> = site.events().iter().map(|e| e["event"].clone()).collect();
        assert_eq!(kinds, [json!("desktop.sign_in_completed")]);

        for good in [
            "2026.11.4",
            "2026.11.4-beta.7",
            "1.4.2+build.7",
            "1.4.2-rc.1+sha.abc",
        ] {
            assert!(valid_version(good), "{good}");
        }
        for bad in [
            "v1.4.2",
            "1.4",
            "1.4.2 beta",
            "1.4.2-",
            "1.4.2-beta..1",
            "123456.1.1",
            "someone@example.com",
            "sk_live_abc123",
            "1.4.2-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "",
        ] {
            assert!(!valid_version(bad), "{bad}");
        }
    }

    /// Says when the first request is on the wire and holds it until
    /// released, so later events queue up behind it.
    struct Held(
        Recorded,
        Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>>,
    );

    impl Transport for Held {
        fn post(&self, url: &str, bearer: Option<&str>, body: Value) -> Result<Answer, String> {
            if let Some((started, release)) = self.1.lock().unwrap().take() {
                let _ = started.send(());
                let _ = release.recv();
            }
            self.0.post(url, bearer, body)
        }
    }

    #[test]
    fn the_gateway_takes_up_to_twenty_events_per_request() {
        let (release, held) = mpsc::channel();
        let (started, on_the_wire) = mpsc::channel();
        let recorded = Recorded::default();
        let site = site_with(
            "2026.11.4",
            recorded.clone(),
            Held(recorded, Mutex::new(Some((started, held)))),
        );
        site.sign_in();
        on_the_wire.recv_timeout(Duration::from_secs(5)).unwrap();
        for _ in 0..25 {
            site.telemetry.sign_in_completed();
        }
        release.send(()).unwrap();
        let sizes: Vec<usize> = site
            .requests()
            .iter()
            .map(|(_, _, body)| body["events"].as_array().unwrap().len())
            .collect();
        assert_eq!(sizes, [1, MAX_EVENTS, 5]);
    }

    #[test]
    fn a_refused_batch_is_dropped_without_a_retry() {
        let site = site("2026.11.4");
        site.recorded.answer(400, None);
        site.sign_in();
        assert_eq!(site.requests().len(), 1);
        site.telemetry.sign_in_completed();
        let requests = site.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[1].2["events"][0]["event"],
            "desktop.sign_in_completed"
        );
    }

    #[test]
    fn a_busy_gateway_is_retried_after_its_wait_up_to_the_attempt_limit() {
        let site = site("2026.11.4");
        site.recorded.answer(429, Some("1"));
        let started = Instant::now();
        site.sign_in();
        assert_eq!(site.requests().len(), 2);
        assert!(started.elapsed() >= Duration::from_secs(1));
        // Busy every time: MAX_ATTEMPTS requests, then the batch is dropped.
        for _ in 0..MAX_ATTEMPTS {
            site.recorded.answer(429, Some("0"));
        }
        site.telemetry.sign_in_completed();
        assert_eq!(site.requests().len(), 2 + MAX_ATTEMPTS as usize);
        site.telemetry.sign_in_completed();
        assert_eq!(site.requests().len(), 3 + MAX_ATTEMPTS as usize);
    }

    #[test]
    fn a_failing_gateway_is_retried_with_backoff() {
        let site = site("2026.11.4");
        site.recorded.answer(503, None);
        let started = Instant::now();
        site.sign_in();
        let requests = site.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].2, requests[1].2);
        assert!(started.elapsed() >= BACKOFF);
    }

    #[test]
    fn an_opt_out_stops_a_retry_wait() {
        let site = site("2026.11.4");
        site.recorded.answer(429, Some("30"));
        site.sign_in();
        let started = Instant::now();
        while site.recorded.0.lock().unwrap().is_empty() {
            assert!(started.elapsed() < Duration::from_secs(5));
            std::thread::yield_now();
        }
        site.telemetry.set_enabled(false).unwrap();
        assert!(site.telemetry.flush(Duration::from_secs(5)));
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(site.recorded.0.lock().unwrap().len(), 1);
    }

    #[test]
    fn a_start_sent_without_a_fresh_session_is_sent_at_the_next_attach() {
        let site = site("2026.11.4");
        // A kept session that is stale at launch: attached, but no bearer.
        site.telemetry.attach(Some(account()));
        assert!(site.requests().is_empty());

        site.sign_in();
        site.telemetry.attach(Some(account()));
        let kinds: Vec<Value> = site.events().iter().map(|e| e["event"].clone()).collect();
        assert_eq!(kinds, [json!("desktop.app_started")]);
    }

    #[test]
    fn a_batch_over_the_body_limit_is_dropped_unsent() {
        let recorded = Recorded::default();
        let mut event = sign_in_completed("2026-11-08T12:00:00Z".into());
        event["properties"] = json!({"padding": "x".repeat(MAX_BODY_BYTES)});
        let delivery = post(&recorded, URL, "session-token", vec![event]);
        assert!(matches!(delivery, Delivery::Dropped(_)), "{delivery:?}");
        assert!(recorded.0.lock().unwrap().is_empty());

        let fits = vec![sign_in_completed("2026-11-08T12:00:00Z".into()); MAX_EVENTS];
        assert_eq!(post(&recorded, URL, "session-token", fits), Delivery::Sent);
    }

    /// `Recorded`, and a channel that disconnects once the sender thread
    /// has let go of it.
    struct Watched {
        recorded: Recorded,
        _held: mpsc::Sender<()>,
    }

    impl Transport for Watched {
        fn post(&self, url: &str, bearer: Option<&str>, body: Value) -> Result<Answer, String> {
            self.recorded.post(url, bearer, body)
        }
    }

    #[test]
    fn a_shutdown_stops_a_retry_wait() {
        let recorded = Recorded::default();
        recorded.answer(429, Some("30"));
        let (held, released) = mpsc::channel();
        let site = site_with(
            "2026.11.4",
            recorded.clone(),
            Watched {
                recorded: recorded.clone(),
                _held: held,
            },
        );
        site.sign_in();
        let started = Instant::now();
        while recorded.0.lock().unwrap().is_empty() {
            assert!(started.elapsed() < Duration::from_secs(5));
            std::thread::yield_now();
        }
        drop(site);
        assert_eq!(
            released.recv_timeout(Duration::from_secs(5)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        );
        assert!(started.elapsed() < Duration::from_secs(10));
        assert_eq!(recorded.0.lock().unwrap().len(), 1);
    }
}
