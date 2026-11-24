use super::*;
fn binding() -> Binding {
    Binding {
        site_url: "https://relay.test".into(),
        workspace_id: "00000000-0000-4000-8000-000000000001".into(),
        account_id: "alice".into(),
    }
}
#[test]
fn persistence_is_private_idempotent_and_account_scoped() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("credentials.json");
    let original = binding();
    let mut saved = Credentials::load(&path, &original).unwrap();
    let grant = Grant::new().unwrap();
    assert_eq!(grant.token.expose().len(), 50);
    saved.grants.insert("grok".into(), grant.clone());
    saved.save(&path).unwrap();
    let loaded = Credentials::load(&path, &original).unwrap();
    assert_eq!(loaded.installation_id, saved.installation_id);
    assert_eq!(loaded.grants["grok"].token, grant.token);
    assert!(agent_relay_node::platform::fs::private_to_owner(
        &std::fs::metadata(&path).unwrap(),
        0o077
    ));
    for changed in [
        Binding {
            account_id: "bob".into(),
            ..original.clone()
        },
        Binding {
            site_url: "https://other.test".into(),
            ..original.clone()
        },
        Binding {
            workspace_id: "other-workspace".into(),
            ..original.clone()
        },
    ] {
        let other = Credentials::load(&path, &changed).unwrap();
        assert!(other.grants.is_empty());
        assert_ne!(other.installation_id, saved.installation_id);
    }
}
#[test]
fn only_selected_confirmed_unexpired_credentials_are_exported() {
    let dir = tempfile::tempdir().unwrap();
    let mut saved = Credentials::load(&dir.path().join("credentials"), &binding()).unwrap();
    let mut grant = Grant::new().unwrap();
    grant.lease_expires = 1_000_000;
    saved.grants.insert("grok".into(), grant.clone());
    let selected = BTreeSet::from(["grok".into()]);
    assert!(saved.active(&selected, 0).is_empty());
    grant.id = Some("credential-id".into());
    saved.grants.insert("grok".into(), grant);
    assert_eq!(saved.active(&selected, 0).len(), 1);
    assert!(saved.active(&BTreeSet::new(), 0).is_empty());
    assert!(saved.active(&selected, 985_001).is_empty());
}
#[test]
fn lease_reply_must_match_provider_workspace_secret_and_existing_id() {
    let binding = binding();
    let mut grant = Grant::new().unwrap();
    grant.id = Some("child-id".into());
    let now = 1_800_000_000_000;
    let reply = json!({"id":"child-id","workspaceId":binding.workspace_id,"provider":"grok","leaseExpiresAt":cloud_session::iso(now+900_000),"accessToken":grant.token.expose()});
    let accepted = remote::accept(&reply, &binding, "grok", &grant, now).unwrap();
    assert_eq!(accepted.token, grant.token);
    for (key, value) in [
        ("id", json!("other-id")),
        ("id", json!("../path")),
        ("workspaceId", json!("other-workspace")),
        ("provider", json!("pi")),
        ("accessToken", json!("wrong-token")),
        ("leaseExpiresAt", json!(cloud_session::iso(now - 1))),
        ("leaseExpiresAt", json!(cloud_session::iso(now + 3_600_000))),
    ] {
        let mut wrong = reply.clone();
        wrong[key] = value;
        assert!(
            remote::accept(&wrong, &binding, "grok", &grant, now).is_none(),
            "{key}"
        );
    }
    let mut renewed = reply;
    renewed.as_object_mut().unwrap().remove("accessToken");
    assert!(remote::accept(&renewed, &binding, "grok", &grant, now).is_some());
}
#[test]
fn malformed_or_insecure_credentials_are_not_replaced_silently() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("credentials");
    std::fs::write(&path, "broken").unwrap();
    agent_relay_node::platform::fs::set_mode(&path, 0o600).unwrap();
    assert!(Credentials::load(&path, &binding()).is_err());
    #[cfg(unix)]
    {
        let valid = Credentials::load(&dir.path().join("new"), &binding()).unwrap();
        agent_relay_node::platform::fs::set_mode(&path, 0o644).unwrap();
        assert!(valid.save(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "broken");
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(Credentials::load(&link, &binding()).is_err());
        assert!(valid.save(&link).is_err());
    }
}

#[test]
fn refused_grants_are_replaced_and_offline_grants_keep_retry_material() {
    let dir = tempfile::tempdir().unwrap();
    let mut saved = Credentials::load(&dir.path().join("credentials"), &binding()).unwrap();
    let mut grant = Grant::new().unwrap();
    grant.id = Some("grant-grok".into());
    grant.lease_expires = 1_000_000;
    saved.grants.insert("grok".into(), grant.clone());
    let selected = BTreeSet::from(["grok".into()]);
    assert!(!saved.needs_network(&selected, 0));
    saved.apply(vec![("grok".into(), remote::Change::Rejected)], 10);
    assert!(!saved.needs_network(&selected, 60_009));
    assert!(saved.needs_network(&selected, 60_010));
    assert!(saved.active(&selected, 10).is_empty());
    // A refused grant is replaced, so the next pass issues a new grant for
    // the current login instead of renewing the refused id forever.
    let fresh = saved.grants["grok"].clone();
    assert_ne!(fresh.token, grant.token);
    assert!(fresh.id.is_none() && !fresh.needs_app_login);
    saved.apply(vec![("grok".into(), remote::Change::Retry)], 70_000);
    assert_eq!(saved.grants["grok"].token, fresh.token);
    assert!(!saved.needs_network(&BTreeSet::new(), 129_999));
    assert!(saved.needs_network(&BTreeSet::new(), 130_000));
}

#[test]
fn sign_in_required_keeps_the_grant_until_a_new_login_renews_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut saved = Credentials::load(&dir.path().join("credentials"), &binding()).unwrap();
    let mut grant = Grant::new().unwrap();
    grant.id = Some("grant-pi".into());
    grant.lease_expires = 1_000_000;
    saved.grants.insert("pi".into(), grant.clone());
    let selected = BTreeSet::from(["pi".into()]);
    saved.apply(vec![("pi".into(), remote::Change::SignInRequired)], 10);
    let kept = &saved.grants["pi"];
    assert_eq!(kept.token, grant.token);
    assert_eq!(kept.id, grant.id);
    assert!(kept.needs_app_login);
    assert!(saved.active(&selected, 10).is_empty());
}

#[test]
fn provider_login_requirement_is_pinned_to_its_account() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let installer = Installer::new(
        home.into(),
        agent_relay_node::relay::codex::CodexConfig::from_env(&|_| None, home),
        agent_relay_node::relay::claude::ClaudeConfig::resolve(&|_| None, home, &[]),
        agent_relay_node::relay::opencode::OpenCodeConfig::from_env(&|_| None, home),
    );
    let pinned = binding();
    assert!(!installer.provider_login_required(&pinned));
    installer.set_provider_login_required(&pinned, vec!["grok".into()]);
    assert!(installer.provider_login_required(&pinned));
    let other = Binding {
        account_id: "bob".into(),
        ..pinned.clone()
    };
    assert!(!installer.provider_login_required(&other));
    installer.set_provider_login_required(&pinned, Vec::new());
    assert!(!installer.provider_login_required(&pinned));
}

#[test]
fn desktop_reauth_refusals_ask_for_a_new_app_login() {
    use remote::{refusal, Change};
    let message = "An Agent Relay app login is required.";
    for body in [
        json!({"error": message, "error_code": "desktop_reauth_required"}),
        json!({"error": "Sign in again.", "error_code": "desktop_reauth_required"}),
    ] {
        for has_id in [false, true] {
            assert!(
                matches!(refusal(403, &body, has_id), Some(Change::SignInRequired)),
                "{body}"
            );
        }
    }
    // The message alone (older Cloud, or a token refreshed mid-request) is
    // retried before it asks for a sign-in.
    for has_id in [false, true] {
        assert!(matches!(
            refusal(403, &json!({"error": message}), has_id),
            Some(Change::AppLoginRefused)
        ));
    }
    // Any other refusal keeps today's meaning.
    for body in [
        json!({"error": "Forbidden"}),
        json!({"error": "Forbidden", "error_code": "workspace_forbidden"}),
        json!({}),
    ] {
        assert!(
            matches!(refusal(403, &body, true), Some(Change::Rejected)),
            "{body}"
        );
    }
    let reauth = json!({"error": message, "error_code": "desktop_reauth_required"});
    assert!(matches!(
        refusal(401, &reauth, false),
        Some(Change::Rejected)
    ));
    assert!(matches!(
        refusal(404, &json!({}), true),
        Some(Change::Remove)
    ));
    assert!(matches!(
        refusal(404, &json!({}), false),
        Some(Change::Rejected)
    ));
    assert!(refusal(200, &reauth, true).is_none());
    assert!(refusal(500, &json!({}), true).is_none());
}

#[test]
fn account_health_asks_for_sign_in_while_providers_need_a_new_app_login() {
    let ours = Some(PROVIDER_SIGN_IN_AGAIN);
    let owned =
        |health: &str, error: Option<&str>| (Some(health.to_string()), error.map(String::from));
    // A working login Cloud refuses for provider credentials is not healthy.
    assert_eq!(
        health_with_provider_sign_in(Some("ready"), None, true),
        owned("sign_in_required", ours)
    );
    // Once the new login issues them, health is ready again.
    assert_eq!(
        health_with_provider_sign_in(Some("sign_in_required"), ours, false),
        owned("ready", None)
    );
    // Other states, and a sign-in required for another reason, are kept.
    let other = Some("Sign in again to restore Agent Relay authentication.");
    assert_eq!(
        health_with_provider_sign_in(Some("sign_in_required"), other, false),
        owned("sign_in_required", other)
    );
    assert_eq!(
        health_with_provider_sign_in(Some("offline"), Some("Retrying"), true),
        owned("offline", Some("Retrying"))
    );
    assert_eq!(health_with_provider_sign_in(None, None, true), (None, None));
    assert_eq!(
        health_with_provider_sign_in(Some("ready"), None, false),
        owned("ready", None)
    );
}

#[test]
fn an_unexplained_app_login_refusal_asks_for_sign_in_only_when_it_repeats() {
    let dir = tempfile::tempdir().unwrap();
    let mut saved = Credentials::load(&dir.path().join("credentials"), &binding()).unwrap();
    saved.grants.insert("pi".into(), Grant::new().unwrap());
    let selected = BTreeSet::from(["pi".into()]);
    saved.apply(vec![("pi".into(), remote::Change::AppLoginRefused)], 10);
    // Retried at once with the current token, without asking for a sign-in.
    assert!(!saved.grants["pi"].needs_app_login);
    assert!(saved.needs_network(&selected, 10));
    saved.apply(vec![("pi".into(), remote::Change::AppLoginRefused)], 20);
    assert!(saved.grants["pi"].needs_app_login);
    assert!(!saved.needs_network(&selected, 20));

    // A refusal followed by an issued grant starts over.
    let mut saved = Credentials::load(&dir.path().join("other"), &binding()).unwrap();
    saved.grants.insert("pi".into(), Grant::new().unwrap());
    saved.apply(vec![("pi".into(), remote::Change::AppLoginRefused)], 10);
    let mut issued = saved.grants["pi"].clone();
    issued.id = Some("grant-pi".into());
    issued.app_login_refused = false;
    saved.apply(vec![("pi".into(), remote::Change::Updated(issued))], 20);
    saved.apply(vec![("pi".into(), remote::Change::AppLoginRefused)], 30);
    assert!(!saved.grants["pi"].needs_app_login);
}

#[test]
fn a_refusal_of_a_superseded_bearer_does_not_ask_for_sign_in() {
    use remote::{for_bearer, refusal, Change};
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    let installer = Installer::new(
        home.into(),
        agent_relay_node::relay::codex::CodexConfig::from_env(&|_| None, home),
        agent_relay_node::relay::claude::ClaudeConfig::resolve(&|_| None, home, &[]),
        agent_relay_node::relay::opencode::OpenCodeConfig::from_env(&|_| None, home),
    );
    let pinned = binding();
    let selected = BTreeSet::from(["pi".into()]);
    let message = "An Agent Relay app login is required.";
    for body in [
        json!({"error": message, "error_code": "desktop_reauth_required"}),
        json!({"error": message}),
    ] {
        let mut saved = Credentials::load(&home.join("credentials"), &pinned).unwrap();
        saved.grants.insert("pi".into(), Grant::new().unwrap());
        // The same refusal twice, each of a bearer a refresh replaced.
        for now in [10, 20] {
            let change = for_bearer(refusal(403, &body, false).unwrap(), false);
            assert!(matches!(change, Change::RetryNow), "{body}");
            saved.apply(vec![("pi".into(), change)], now);
        }
        let grant = &saved.grants["pi"];
        assert!(!grant.needs_app_login && !grant.app_login_refused, "{body}");
        // Retried at once with the current token.
        assert!(saved.needs_network(&selected, 20));
        saved.publish(&installer, &pinned, &selected, 20);
        assert!(!installer.provider_login_required(&pinned), "{body}");
        // Health therefore stays ready.
        assert_eq!(
            health_with_provider_sign_in(Some("ready"), None, false),
            (Some("ready".into()), None)
        );
    }
    // The current bearer's refusal still counts, and other answers pass through.
    let reauth = json!({"error": message, "error_code": "desktop_reauth_required"});
    assert!(matches!(
        for_bearer(refusal(403, &reauth, false).unwrap(), true),
        Change::SignInRequired
    ));
    assert!(matches!(
        for_bearer(Change::Rejected, false),
        Change::Rejected
    ));
}
