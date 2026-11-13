//! Where telemetry goes and how it gets there: a PostHog project (an
//! ingestion key and an https origin, validated before anything is sent) and
//! the sender that posts one capture event per request to it, or batches
//! of a site's own analytics events to that site's gateway.
//!
//! A production build sends to Agent Relay's public ingestion host unless
//! its release environment names another (`AGENT_RELAY_POSTHOG_HOST`), and
//! only with a key (`AGENT_RELAY_POSTHOG_API_KEY`); source and development
//! builds contain no key, so they never send.
//!
//! A build made for its own site (`site_build`) never sends to the public
//! host. By default it sends only its app-started and sign-in-completed
//! events to its own site's analytics gateway (`gateway`), with the
//! signed-in session as `Authorization: Bearer`; before sign-in they are
//! dropped, and no capture event or `$identify` goes there. Where the site
//! exports them is the site's choice, which Desktop never learns. A site
//! build always sends to its own site's analytics gateway;
//! `AGENT_RELAY_POSTHOG_HOST` and `AGENT_RELAY_POSTHOG_API_KEY` are refused
//! when it is made (`build_site_telemetry.rs`). A site build whose site
//! cannot be read sends nothing.

use super::gateway::{self, Delivery};
use super::{json, Value, SEND_TIMEOUT};
use agent_relay_node::relay::workspace::Secret;
use std::sync::{Arc, OnceLock, Weak};

mod https;
use https::Https;

/// Agent Relay's public PostHog ingestion host.
pub const DEFAULT_HOST: &str = "https://i.agentrelay.com";

/// Where a site's analytics gateway takes Desktop's events, below the
/// site's origin. The sender posts to exactly this URL, appending nothing.
pub const SITE_COLLECTOR_PATH: &str = gateway::PATH;

/// A PostHog project: an ingestion key and an https origin, or a site's
/// own collector, which also needs the signed-in session.
#[derive(Clone, PartialEq, Eq)]
pub struct PostHogProject {
    pub(super) api_key: String,
    host: String,
    on_site: bool,
}

impl std::fmt::Debug for PostHogProject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PostHogProject")
            .field("host", &self.host)
            .field("on_site", &self.on_site)
            .finish_non_exhaustive()
    }
}

/// An https origin: no credentials, query, fragment or path.
fn https_origin(text: &str) -> Option<url::Url> {
    let url = url::Url::parse(text).ok()?;
    (url.scheme() == "https"
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && (url.path().is_empty() || url.path() == "/"))
        .then_some(url)
}

impl PostHogProject {
    pub fn new(api_key: &str, host: &str) -> Option<PostHogProject> {
        let key_ok = api_key.starts_with("phc_")
            && api_key.len() < 256
            && api_key
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
        let url = https_origin(host)?;
        key_ok.then(|| PostHogProject {
            api_key: api_key.to_string(),
            host: url.origin().ascii_serialization(),
            on_site: false,
        })
    }

    /// `site`'s analytics gateway ([`SITE_COLLECTOR_PATH`]). It binds events
    /// to the bearer's session and takes no project key.
    pub fn site_collector(site: &str) -> Option<PostHogProject> {
        let url = https_origin(site)?;
        Some(PostHogProject {
            api_key: String::new(),
            host: format!(
                "{}{SITE_COLLECTOR_PATH}",
                url.origin().ascii_serialization()
            ),
            on_site: true,
        })
    }

    /// The release project. Debug builds and production builds without the
    /// release key have none, so forks, development and tests never send.
    /// `AGENT_RELAY_POSTHOG_HOST` overrides the default host of a production
    /// build; a site build always uses its own site's gateway, and the build
    /// refuses a PostHog host or key for it when it is made (see
    /// [`PostHogProject::bundled_for`]).
    pub fn bundled() -> Option<PostHogProject> {
        if cfg!(debug_assertions) {
            None
        } else {
            PostHogProject::bundled_for(
                agent_relay_node::relay::links::built_in_site_url(),
                option_env!("AGENT_RELAY_POSTHOG_API_KEY"),
                option_env!("AGENT_RELAY_POSTHOG_HOST"),
            )
        }
    }

    /// The release project for a build made for `site` with `api_key` and
    /// `host` from its release environment. A site build always sends to its
    /// own site's analytics gateway: `AGENT_RELAY_POSTHOG_HOST` and
    /// `AGENT_RELAY_POSTHOG_API_KEY` are refused when it is made, and should
    /// `host` or `api_key` still say something, they are never used. A
    /// production build with a named `host` uses that PostHog project, and
    /// without one [`DEFAULT_HOST`].
    pub(super) fn bundled_for(
        site: &str,
        api_key: Option<&str>,
        host: Option<&str>,
    ) -> Option<PostHogProject> {
        if crate::site_build::is_site_build_for(site) {
            return PostHogProject::site_collector(site);
        }
        PostHogProject::new(api_key?, host.unwrap_or(DEFAULT_HOST))
    }

    /// The origin, without a trailing slash; for a site's gateway, its
    /// whole URL (the origin and [`SITE_COLLECTOR_PATH`]).
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Whether this is a site's analytics gateway, which takes only its own
    /// events and only with the signed-in session.
    pub fn on_site(&self) -> bool {
        self.on_site
    }
}

/// The signed-in session's access token, read when an event is sent.
pub trait SessionBearer: Send + Sync {
    /// `None` when nobody is signed in (or the session is not usable now).
    fn bearer(&self) -> Option<Secret>;
}

/// Where a site collector's sender finds the session: set once, when the
/// account exists, and held weakly so the account owns telemetry and not
/// the other way round.
pub type SessionSlot = Arc<OnceLock<Weak<dyn SessionBearer>>>;

/// How a server answered a POST.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Answer {
    pub status: u16,
    /// The `Retry-After` header, as sent.
    pub retry_after: Option<String>,
}

/// A local refusal cannot recover by retrying the same telemetry batch.
#[derive(Debug)]
pub enum TransportFailure {
    Permanent(String),
    Transient(String),
}

impl TransportFailure {
    fn reason(self) -> String {
        match self {
            Self::Permanent(reason) | Self::Transient(reason) => reason,
        }
    }
}

/// One POST of a JSON body with an optional bearer: the answer, or `Err`
/// when there was none. The seam that keeps [`PostHogSender`] testable
/// without a server.
pub trait Transport: Send + Sync {
    fn post(&self, url: &str, bearer: Option<&str>, body: Value) -> Result<Answer, String>;

    fn checked_post(
        &self,
        url: &str,
        bearer: Option<&str>,
        body: Value,
    ) -> Result<Answer, TransportFailure> {
        self.post(url, bearer, body)
            .map_err(TransportFailure::Transient)
    }
}

/// PostHog's capture endpoint (`/i/v0/e/`) below the project's host, one
/// event per request; or a site's analytics gateway, batches of its own
/// events (`gateway`). The gateway gets the signed-in session as the
/// bearer; with none, the events are dropped, never sent without it and
/// never kept for later.
pub struct PostHogSender {
    project: PostHogProject,
    session: SessionSlot,
    transport: Box<dyn Transport>,
}

impl PostHogSender {
    pub fn new(project: PostHogProject, session: SessionSlot) -> PostHogSender {
        PostHogSender::with_transport(project, session, Https::new())
    }

    pub fn with_transport(
        project: PostHogProject,
        session: SessionSlot,
        transport: impl Transport + 'static,
    ) -> PostHogSender {
        PostHogSender {
            project,
            session,
            transport: Box::new(transport),
        }
    }

    /// The bearer a site's gateway needs, read now; `None` when nobody is
    /// signed in.
    fn bearer(&self) -> Option<Secret> {
        self.session
            .get()
            .and_then(Weak::upgrade)
            .and_then(|session| session.bearer())
    }
}

impl super::TelemetrySender for PostHogSender {
    fn send(&self, batch: Vec<Value>) -> Result<(), String> {
        if self.project.on_site {
            return self.deliver(batch).into_result();
        }
        let url = format!("{}/i/v0/e/", self.project.host);
        let mut outcome = Ok(());
        for event in batch {
            let mut body = json!({"api_key": self.project.api_key});
            if let (Value::Object(body), Value::Object(event)) = (&mut body, event) {
                body.extend(event);
            }
            let sent = delivered(self.transport.post(&url, None, body));
            if outcome.is_ok() {
                outcome = sent;
            }
        }
        outcome
    }

    fn batch_limit(&self) -> usize {
        if self.project.on_site {
            gateway::MAX_EVENTS
        } else {
            1
        }
    }

    fn deliver(&self, batch: Vec<Value>) -> Delivery {
        if !self.project.on_site {
            return match self.send(batch) {
                Ok(()) => Delivery::Sent,
                Err(reason) => Delivery::Dropped(reason),
            };
        }
        match self.bearer() {
            Some(bearer) => gateway::post(
                self.transport.as_ref(),
                &self.project.host,
                bearer.expose(),
                batch,
            ),
            None => Delivery::Dropped("Not signed in: telemetry for the site was dropped.".into()),
        }
    }
}

/// Whether PostHog took a batch: only a 2xx counts, so a redirected
/// `$identify` is never recorded as merged.
fn delivered(answer: Result<Answer, String>) -> Result<(), String> {
    match answer?.status {
        200..=299 => Ok(()),
        status => Err(format!("PostHog answered {status}.")),
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::super::TelemetrySender;
    use super::*;
    use agent_relay_node::relay::links::DEFAULT_SITE_URL;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    const SITE: &str = "https://site.example";

    fn host(project: Option<PostHogProject>) -> Option<String> {
        project.map(|project| project.host().to_string())
    }

    #[test]
    fn a_production_build_defaults_to_the_public_host() {
        let project = PostHogProject::bundled_for(DEFAULT_SITE_URL, Some("phc_test"), None);
        assert_eq!(host(project.clone()), Some(DEFAULT_HOST.to_string()));
        assert!(!project.unwrap().on_site());
        assert_eq!(
            host(PostHogProject::bundled_for(
                DEFAULT_SITE_URL,
                Some("phc_test"),
                Some("https://analytics.example.com")
            )),
            Some("https://analytics.example.com".to_string())
        );
        assert!(PostHogProject::bundled_for(DEFAULT_SITE_URL, None, None).is_none());
    }

    #[test]
    fn a_site_build_defaults_to_its_own_collector() {
        let project = PostHogProject::bundled_for(SITE, None, None).unwrap();
        assert_eq!(
            project.host(),
            "https://site.example/api/v1/analytics/events"
        );
        assert!(project.on_site());
        let keyed = PostHogProject::bundled_for(&format!("{SITE}/"), Some("phc_test"), None);
        assert_eq!(keyed, Some(project));
        // A site that cannot be read sends nothing, and never falls back.
        for site in [
            "http://site.example",
            "https://user@site.example",
            "not a url",
        ] {
            assert!(
                PostHogProject::bundled_for(site, Some("phc_test"), None).is_none(),
                "{site}"
            );
        }
    }

    #[test]
    fn a_site_build_never_uses_a_named_posthog_host() {
        let own = PostHogProject::bundled_for(SITE, None, None).unwrap();
        assert!(own.on_site());
        for api_key in [Some("phc_test"), None] {
            for host in [
                "https://analytics.example.com/",
                "http://analytics.example.com",
            ] {
                let project = PostHogProject::bundled_for(SITE, api_key, Some(host)).unwrap();
                assert!(project.on_site(), "{api_key:?} {host}");
                assert_eq!(
                    project.host(),
                    "https://site.example/api/v1/analytics/events"
                );
                assert_eq!(project, own);
            }
        }
    }

    /// One request: the URL, the bearer and the body.
    pub(in crate::telemetry) type Request = (String, Option<String>, Value);

    /// Every request. Answers come from the script in order, then `202`.
    #[derive(Clone, Default)]
    pub(in crate::telemetry) struct Recorded(
        pub Arc<Mutex<Vec<Request>>>,
        pub Arc<Mutex<VecDeque<Result<Answer, String>>>>,
    );

    impl Recorded {
        pub(in crate::telemetry) fn answer(&self, status: u16, retry_after: Option<&str>) {
            self.1.lock().unwrap().push_back(Ok(Answer {
                status,
                retry_after: retry_after.map(str::to_string),
            }));
        }
    }

    impl Transport for Recorded {
        fn post(&self, url: &str, bearer: Option<&str>, body: Value) -> Result<Answer, String> {
            let request = (url.to_string(), bearer.map(str::to_string), body);
            self.0.lock().unwrap().push(request);
            self.1.lock().unwrap().pop_front().unwrap_or(Ok(Answer {
                status: 202,
                retry_after: None,
            }))
        }
    }

    pub(in crate::telemetry) struct Signed(pub Mutex<Option<&'static str>>);

    impl SessionBearer for Signed {
        fn bearer(&self) -> Option<Secret> {
            self.0.lock().unwrap().map(Secret::new)
        }
    }

    pub(in crate::telemetry) fn sender(
        project: PostHogProject,
        token: Option<&'static str>,
    ) -> (PostHogSender, Recorded, Arc<Signed>) {
        let recorded = Recorded::default();
        let signed = Arc::new(Signed(Mutex::new(token)));
        let slot = SessionSlot::default();
        let weak: Weak<dyn SessionBearer> = Arc::downgrade(&signed) as Weak<dyn SessionBearer>;
        assert!(slot.set(weak).is_ok());
        let sender = PostHogSender::with_transport(project, slot, recorded.clone());
        (sender, recorded, signed)
    }

    fn event() -> Value {
        json!({"event": "desktop_operation", "distinct_id": "user_123", "properties": {"operation": "app_launch"}})
    }

    #[test]
    fn the_site_gateway_takes_one_client_batch_with_the_session() {
        let project = PostHogProject::bundled_for(SITE, None, None).unwrap();
        let (sender, recorded, _signed) = sender(project, Some("session-token"));
        let events = vec![
            gateway::sign_in_completed("2026-11-08T12:00:00Z".into()),
            gateway::sign_in_completed("2026-11-08T12:00:01Z".into()),
        ];
        assert_eq!(sender.batch_limit(), gateway::MAX_EVENTS);
        sender.send(events.clone()).unwrap();
        let requests = recorded.0.lock().unwrap().clone();
        assert_eq!(requests.len(), 1);
        let (url, bearer, body) = &requests[0];
        assert_eq!(url, "https://site.example/api/v1/analytics/events");
        assert_eq!(bearer.as_deref(), Some("session-token"));
        assert_eq!(
            body,
            &json!({
                "schema": "agent-relay.analytics.client-batch/v1",
                "client": "desktop",
                "events": events,
            })
        );
        assert!(!body.to_string().contains("session-token"));
    }

    #[test]
    fn the_site_gateway_answers_decide_between_done_dropped_and_retry() {
        let project = PostHogProject::bundled_for(SITE, None, None).unwrap();
        let (sender, recorded, _signed) = sender(project, Some("session-token"));
        let batch = || vec![gateway::sign_in_completed("2026-11-08T12:00:00Z".into())];
        let retry = |after: Option<u64>| Delivery::Retry {
            reason: String::new(),
            after: after.map(std::time::Duration::from_secs),
        };
        let cases: Vec<(u16, Option<&str>, Delivery)> = vec![
            (202, None, Delivery::Sent),
            (200, None, Delivery::Dropped(String::new())),
            (204, None, Delivery::Dropped(String::new())),
            (302, None, Delivery::Dropped(String::new())),
            (400, None, Delivery::Dropped(String::new())),
            (401, None, Delivery::Dropped(String::new())),
            (403, None, Delivery::Dropped(String::new())),
            (404, None, Delivery::Dropped(String::new())),
            (408, None, Delivery::Dropped(String::new())),
            (413, None, Delivery::Dropped(String::new())),
            (429, Some("2"), retry(Some(2))),
            (429, Some("0"), retry(Some(0))),
            (429, Some("120"), retry(Some(30))),
            (429, Some("99999999999999999999999"), retry(Some(30))),
            (429, Some("Wed, 21 Oct 2026 07:28:00 GMT"), retry(None)),
            (429, Some("-1"), retry(None)),
            (429, Some("1.5"), retry(None)),
            (429, None, retry(None)),
            (500, None, retry(None)),
            (503, Some("5"), retry(None)),
        ];
        let blank = |delivery: Delivery| match delivery {
            Delivery::Dropped(_) => Delivery::Dropped(String::new()),
            Delivery::Retry { after, .. } => Delivery::Retry {
                reason: String::new(),
                after,
            },
            sent => sent,
        };
        for (status, retry_after, expected) in cases {
            recorded.answer(status, retry_after);
            assert_eq!(
                blank(sender.deliver(batch())),
                expected,
                "{status} {retry_after:?}"
            );
        }
        recorded
            .1
            .lock()
            .unwrap()
            .push_back(Err("unreachable".into()));
        assert_eq!(blank(sender.deliver(batch())), retry(None));
        // An empty batch makes no request.
        let before = recorded.0.lock().unwrap().len();
        assert_eq!(sender.deliver(vec![]), Delivery::Sent);
        assert_eq!(recorded.0.lock().unwrap().len(), before);
    }

    #[test]
    fn a_posthog_host_keeps_one_capture_per_request_and_any_2xx() {
        let project = PostHogProject::bundled_for(
            DEFAULT_SITE_URL,
            Some("phc_test"),
            Some("https://analytics.example.com"),
        )
        .unwrap();
        let (sender, recorded, _signed) = sender(project, None);
        assert_eq!(sender.batch_limit(), 1);
        recorded.answer(200, None);
        assert_eq!(sender.deliver(vec![event()]), Delivery::Sent);
        recorded.answer(503, None);
        assert_eq!(
            sender.deliver(vec![event()]),
            Delivery::Dropped("PostHog answered 503.".into())
        );
    }

    #[test]
    fn the_site_collector_gets_nothing_without_a_session() {
        let project = PostHogProject::bundled_for(SITE, None, None).unwrap();
        let (sender, recorded, signed) = sender(project.clone(), None);
        assert!(sender.send(vec![event()]).is_err());
        assert!(recorded.0.lock().unwrap().is_empty());
        // Nothing was kept: a later sign-in sends only what comes next.
        *signed.0.lock().unwrap() = Some("session-token");
        sender.send(vec![]).unwrap();
        assert!(recorded.0.lock().unwrap().is_empty());
        // No account behind the slot is no session either.
        let lone = PostHogSender::with_transport(project, SessionSlot::default(), recorded.clone());
        assert!(lone.send(vec![event()]).is_err());
        assert!(recorded.0.lock().unwrap().is_empty());
    }

    #[test]
    fn a_posthog_host_gets_no_session() {
        let project = PostHogProject::bundled_for(
            DEFAULT_SITE_URL,
            Some("phc_test"),
            Some("https://analytics.example.com"),
        )
        .unwrap();
        let (sender, recorded, _signed) = sender(project, Some("session-token"));
        sender.send(vec![event()]).unwrap();
        let requests = recorded.0.lock().unwrap().clone();
        assert_eq!(requests[0].0, "https://analytics.example.com/i/v0/e/");
        assert_eq!(requests[0].1, None);
        assert_eq!(requests[0].2["api_key"], "phc_test");
        assert!(!requests[0].2.to_string().contains("session-token"));
    }
}
