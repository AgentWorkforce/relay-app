//! The session bridge (`account::session_bridge`) against the recovery
//! tests' fake Cloud and Relaycast. Throwaway tokens only. A control plane
//! session is put in the store directly where the point is that this
//! process did not make the crossing itself (a restart, or another process
//! sharing the store).
use super::*;
use crate::account::session_bridge::{CHECK_ATTEMPTS, FLAG};
use agent_relay_node::relay::protocol::{Event, Topic};
use tokio::sync::broadcast::{error::TryRecvError, Receiver};

const CONTROL_ACCESS: &str = "sh_at_test-only-access";
const MOVED: &str = "Your organization moved to its new Agent Relay control plane";

fn flag(on: bool) -> BTreeMap<String, String> {
    match on {
        true => BTreeMap::from([(FLAG.to_string(), "1".to_string())]),
        false => BTreeMap::new(),
    }
}

async fn bridged(on: bool) -> Harness {
    Harness::with_env(flag(on)).await
}

impl Harness {
    /// Keeps `access` as the session, as another writer would.
    fn keep_access(&self, access: &str, expires: &str) {
        let site = self.fake.site.clone();
        self.core
            .store
            .update(|s| {
                s.cloud_session = Some(CloudSession {
                    access_token: Secret::new(access),
                    refresh_token: Some(Secret::new("old-refresh")),
                    site_url: site,
                    access_token_expires_at: Some(expires.into()),
                    refresh_token_expires_at: None,
                })
            })
            .unwrap();
    }

    /// Has the resolve answer the stored binding unchanged.
    fn resolve_unchanged(&self) {
        let site = self.fake.site.clone();
        self.fake.state.lock().unwrap().resolved = json!({"data":{"relaycastApiKey": OLD,
            "relaycastWorkspaceId":"rw_review", "cloudWorkspaceId":CLOUD_ID,
            "urls":{"relaycastUrl":site}}});
    }

    fn binding(&self) -> CloudWorkspace {
        self.core.store.read(|s| s.cloud.clone().unwrap())
    }

    fn kept_access(&self) -> String {
        self.core.store.read(|s| {
            s.cloud_session
                .as_ref()
                .unwrap()
                .access_token
                .expose()
                .to_string()
        })
    }
}

/// The activity messages at `level` posted since the last call.
fn activity(events: &mut Receiver<Event>, level: &str) -> Vec<String> {
    let mut found = vec![];
    loop {
        match events.try_recv() {
            Ok(event) if event.topic == Topic::Activity && event.data["level"] == level => {
                found.push(event.data["message"].as_str().unwrap_or("").to_string())
            }
            Ok(_) | Err(TryRecvError::Lagged(_)) => {}
            Err(_) => return found,
        }
    }
}

fn moved(messages: &[String]) -> usize {
    messages.iter().filter(|m| m.starts_with(MOVED)).count()
}

/// Off, a control plane session is never checked.
#[tokio::test]
async fn off_a_control_plane_session_is_not_checked() {
    let h = bridged(false).await;
    h.keep_access(CONTROL_ACCESS, "2099-01-01T00:00:00Z");
    let mut events = h.core.hub.subscribe();
    h.account.check_bridged_binding().await;
    assert_eq!(h.fake.count("/resolve"), 0);
    assert_eq!(moved(&activity(&mut events, "warning")), 0);
}

/// On, a control plane session this process did not rotate into (kept by
/// another writer, or found at start) is checked once; a Cloud session
/// never is.
#[tokio::test]
async fn on_a_control_plane_session_kept_by_another_writer_is_checked_once() {
    let h = bridged(true).await;
    h.resolve_unchanged();
    h.account.check_bridged_binding().await;
    assert_eq!(
        h.fake.count("/resolve"),
        0,
        "a Cloud session is not checked"
    );
    h.keep_access(CONTROL_ACCESS, "2099-01-01T00:00:00Z");
    let mut events = h.core.hub.subscribe();
    h.account.check_bridged_binding().await;
    h.account.check_bridged_binding().await;
    assert_eq!(h.fake.count("/resolve"), 1);
    assert_eq!(moved(&activity(&mut events, "warning")), 0);
    // Replaced by a Cloud session and back: checked again.
    h.keep_access("old-access", "2099-01-01T00:00:00Z");
    h.account.check_bridged_binding().await;
    h.keep_access(CONTROL_ACCESS, "2099-01-01T00:00:00Z");
    h.account.check_bridged_binding().await;
    assert_eq!(h.fake.count("/resolve"), 2);
}

/// A resolve that differs from the stored binding is reported and never
/// adopted.
#[tokio::test]
async fn on_a_changed_binding_is_reported_and_left_as_it_is() {
    let h = bridged(true).await;
    // The default resolve shares a different key.
    h.keep_access(CONTROL_ACCESS, "2099-01-01T00:00:00Z");
    let before = h.binding();
    let mut events = h.core.hub.subscribe();
    h.account.check_bridged_binding().await;
    let warnings = activity(&mut events, "warning");
    assert_eq!(moved(&warnings), 1, "{warnings:?}");
    assert!(
        warnings[0].contains("different Agent Relay key"),
        "{warnings:?}"
    );
    assert_eq!(h.binding(), before);
    assert!(h.cloud_key_is(OLD));
}

/// A check Cloud could not answer is tried `CHECK_ATTEMPTS` times in all,
/// and the last failure is reported.
#[tokio::test]
async fn on_an_unanswered_check_is_tried_a_bounded_number_of_times_then_reported() {
    let h = bridged(true).await;
    h.fake.state.lock().unwrap().drop_resolve = true;
    h.keep_access(CONTROL_ACCESS, "2099-01-01T00:00:00Z");
    let mut events = h.core.hub.subscribe();
    for _ in 0..CHECK_ATTEMPTS - 1 {
        h.account.check_bridged_binding().await;
    }
    assert_eq!(moved(&activity(&mut events, "warning")), 0);
    h.account.check_bridged_binding().await;
    let warnings = activity(&mut events, "warning");
    assert_eq!(moved(&warnings), 1, "{warnings:?}");
    assert!(warnings[0].contains("could not be checked"), "{warnings:?}");
    h.account.check_bridged_binding().await;
    assert_eq!(h.fake.count("/resolve"), CHECK_ATTEMPTS as usize);
}

/// The refresh that crosses to the control plane is announced with the
/// flag on, and stored the same either way.
#[tokio::test]
async fn a_refresh_into_the_control_plane_is_kept_either_way_and_announced_only_on() {
    for on in [false, true] {
        let h = bridged(on).await;
        h.fake.state.lock().unwrap().refresh_access = CONTROL_ACCESS.into();
        h.keep_access("old-access", "2000-01-01T00:00:00Z");
        let mut events = h.core.hub.subscribe();
        let bearer = h
            .account
            .device_token()
            .await
            .map(|t| t.expose().to_string());
        assert_eq!(bearer.as_deref(), Some(CONTROL_ACCESS), "on={on}");
        assert_eq!(h.kept_access(), CONTROL_ACCESS, "on={on}");
        assert_eq!(h.fake.count("/auth/token/refresh"), 1, "on={on}");
        assert_eq!(
            moved(&activity(&mut events, "info")),
            usize::from(on),
            "on={on}"
        );
    }
}

/// The renewal refused-key recovery starts from goes through the same
/// rotation: the crossing is announced with the flag on, and recovery
/// itself is unchanged either way.
#[tokio::test]
async fn recovery_renewing_into_the_control_plane_announces_it_only_on() {
    for on in [false, true] {
        let h = bridged(on).await;
        h.fake.state.lock().unwrap().refresh_access = CONTROL_ACCESS.into();
        let refused = h.core.discover().unwrap();
        let mut events = h.core.hub.subscribe();
        let recovered = h.account.recover_workspace_key(&refused, "rw_review").await;
        assert!(recovered.is_some(), "on={on}");
        assert!(h.cloud_key_is(NEW), "on={on}");
        assert_eq!(h.kept_access(), CONTROL_ACCESS, "on={on}");
        let info = activity(&mut events, "info");
        assert_eq!(moved(&info), usize::from(on), "on={on} {info:?}");
    }
}

/// On the control plane, refused-key recovery still refuses a resolve that
/// moves the Relaycast origin (even to one accepting the new key) or names
/// a gate, and keeps the binding: the bridge relies on it.
#[tokio::test]
async fn recovery_on_the_control_plane_refuses_a_moved_origin_or_a_gate() {
    let elsewhere = Fake::start().await;
    for moved_to in ["origin", "gate"] {
        let h = bridged(true).await;
        {
            let mut state = h.fake.state.lock().unwrap();
            state.refresh_access = "sh_at_test-only-renewed".into();
            match moved_to {
                "origin" => {
                    state.resolved["data"]["urls"]["relaycastUrl"] = json!(elsewhere.site.clone())
                }
                _ => state.resolved["data"]["urls"]["relaygateUrl"] = json!(h.fake.site.clone()),
            }
        }
        h.keep_access(CONTROL_ACCESS, "2099-01-01T00:00:00Z");
        let before = h.binding();
        let refused = h.core.discover().unwrap();
        assert!(
            h.account
                .recover_workspace_key(&refused, "rw_review")
                .await
                .is_none(),
            "{moved_to}"
        );
        assert_eq!(h.binding(), before, "{moved_to}");
        assert_eq!(elsewhere.new_gets(), 0, "{moved_to}");
    }
}
