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
