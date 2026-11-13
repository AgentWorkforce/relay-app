//! The outbound tests that need a real account: sign-in state, uploader
//! status and setup.

use agent_relay_desktop_core::account::Account;
#[path = "support"]
mod support {
    pub mod bridge;
    pub mod fake_relaycast;
}

fn account(config: &Config, probe: &Arc<dyn ProbeRunner>) -> Arc<Account> {
    Arc::new(
        Account::new(
            &config.env,
            &config.app_data_dir,
            Arc::new(Uploader::new(probe.clone())),
        )
        .with_site(CLOUD_SITE)
        .unwrap(),
    )
}

include!("support/outbound_harness.rs");

#[tokio::test]
async fn setup_status_names_the_signed_in_account() {
    let mut installs = stalled_install();
    installs["installs"][0]["account_name"] = json!("Ada Lovelace");
    installs["installs"][0]["account_email"] = json!("ada@example.com");
    let probe = Arc::new(StatusProbe {
        status: Mutex::new(json!({"running": true, "paused": false, "sharing_mode": "new"})),
        installs,
    });
    let h = Harness::start_complete(true, false, Some(probe as Arc<dyn ProbeRunner>)).await;
    eventually("the account signed in", || {
        h.core.status().account.state == SignInState::SignedIn
    })
    .await;
    let status = status_promptly(&h).await;
    assert_eq!(
        status["account"],
        json!({"name": "Ada Lovelace", "email": "ada@example.com"})
    );
    h.stop().await;

    let h = Harness::start().await;
    assert_eq!(status_promptly(&h).await["account"], Value::Null);
    h.stop().await;
}

#[tokio::test]
async fn setup_status_reports_the_upload_stage_failure_and_last_acknowledgement() {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let acknowledged = now - 10 * 60 * 1_000;
    let probe = Arc::new(StatusProbe {
        status: Mutex::new(
            json!({"running": true, "paused": false, "sharing_mode": "new",
            "delivery": {"bootstrap_complete": true, "pending_records": 4, "acknowledged_records": 9,
                "last_acknowledged_ms": now - 1_000, "oldest_pending_ms": now - 1_000,
                "failure": null},
            "sessions": {"shared": 1, "total": 1}}),
        ),
        installs: stalled_install(),
    });
    let h = Harness::start_complete(true, false, Some(probe.clone() as Arc<dyn ProbeRunner>)).await;
    let uploader = uploader_status(&h, |u| u["stage"] == "uploading").await;
    assert_eq!(uploader["healthy"], true);
    assert_eq!(uploader["stalled"], false);
    assert_eq!(uploader["failure_code"], Value::Null);
    assert_eq!(uploader["last_acknowledged_ms"], now - 1_000);

    // The same queue, unacknowledged for ten minutes.
    {
        let mut status = probe.status.lock().unwrap();
        status["delivery"]["last_acknowledged_ms"] = json!(acknowledged);
        status["delivery"]["oldest_pending_ms"] = json!(acknowledged);
    }
    let uploader = uploader_status(&h, |u| u["stalled"] == true).await;
    assert_eq!(uploader["stage"], "needs_attention");
    assert_eq!(uploader["healthy"], false);
    assert_eq!(uploader["last_acknowledged_ms"], acknowledged);

    // Blocked on sign-in that Cloud refused to renew.
    {
        let mut status = probe.status.lock().unwrap();
        status["delivery"]["failure"] =
            json!("Agent Relay Cloud refused to renew the upload sign-in.");
        status["delivery"]["failure_code"] = json!("authentication_required");
        status["delivery"]["renewal_failure"] = json!("cloud_refused");
    }
    let uploader = uploader_status(&h, |u| !u["failure_code"].is_null()).await;
    assert_eq!(uploader["failure_code"], "authentication_required");
    assert_eq!(uploader["renewal_failure"], "cloud_refused");
    assert_eq!(uploader["stage"], "needs_attention");
    assert_eq!(uploader["healthy"], false);
    assert_eq!(uploader["pending_records"], 4);
    h.stop().await;
}

#[tokio::test]
async fn status_answers_while_the_uploader_is_stalled() {
    let probe = Arc::new(StalledProbe::default());
    let h = Harness::start_complete(true, true, Some(probe.clone() as Arc<dyn ProbeRunner>)).await;
    eventually("the account signed in", || {
        h.core.status().account.state == SignInState::SignedIn
    })
    .await;
    let status = status_promptly(&h).await;
    assert_eq!(status["signed_in"], true);
    assert_eq!(status["auto_activate"], true);
    assert_eq!(status["direct_delivery"], true);
    assert_eq!(status["defaults"], "pending");
    assert_eq!(status["uploader"]["pending"], true);
    eventually("the bootstrap asking the stalled uploader", || {
        probe.called("status")
    })
    .await;
    // Every later read, and an explicit local choice, still answer.
    let again = status_promptly(&h).await;
    assert_eq!(again["uploader"]["pending"], true);
    let choice = tokio::time::timeout(
        QUICK,
        h.call(
            IN_CLAUDE,
            "POST",
            "/setup/auto-activate",
            json!({"enabled": false}),
        ),
    )
    .await
    .expect("auto-activation answers")
    .unwrap();
    assert_eq!(choice, json!({"auto_activate": false}));
    assert_eq!(status_promptly(&h).await["defaults"], "applied");
    h.stop().await;
}

#[tokio::test]
async fn the_bootstrap_keeps_an_activation_failure_it_did_not_record() {
    let (release, held) = tokio::sync::watch::channel(false);
    let probe = Arc::new(HeldStatusProbe {
        release: held,
        mode: Mutex::new("selected"),
        reads: Mutex::new(0),
        sharing: Mutex::new(0),
        answered: Arc::default(),
    });
    let h =
        Harness::start_complete(false, false, Some(probe.clone() as Arc<dyn ProbeRunner>)).await;
    eventually("the account signed in", || {
        h.core.status().account.state == SignInState::SignedIn
    })
    .await;
    h.fake.set_failing(&["POST /v1/agents"]);
    status_promptly(&h).await;
    eventually("the bootstrap's held uploader read", || {
        *probe.reads.lock().unwrap() > 0
    })
    .await;
    let failed = tokio::time::timeout(
        QUICK,
        h.call(
            IN_CLAUDE,
            "POST",
            "/setup/auto-activate",
            json!({"enabled": true}),
        ),
    )
    .await
    .expect("auto-activation answers");
    assert_eq!(code(failed), (502, "activation_failed".into()));
    release.send(true).unwrap();
    eventually("the held uploader read answered", || {
        probe.answered.load(std::sync::atomic::Ordering::SeqCst)
    })
    .await;
    // The bootstrap's remaining step is local and runs once it is polled.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let status = status_promptly(&h).await;
    assert_eq!(status["defaults"], "applied");
    assert_eq!(status["defaults_error"]["code"], "activation_failed");
    assert_eq!(*probe.sharing.lock().unwrap(), 0);
    h.stop().await;
}

#[tokio::test]
async fn the_bootstrap_keeps_an_installs_own_sharing_mode() {
    for mode in ["new", "all", "selected"] {
        let (_release, held) = tokio::sync::watch::channel(true);
        let probe = Arc::new(HeldStatusProbe {
            release: held,
            mode: Mutex::new(mode),
            reads: Mutex::new(0),
            sharing: Mutex::new(0),
            answered: Arc::default(),
        });
        let h = Harness::start_complete(false, false, Some(probe.clone() as Arc<dyn ProbeRunner>))
            .await;
        eventually("the account signed in", || {
            h.core.status().account.state == SignInState::SignedIn
        })
        .await;
        let deadline = Instant::now() + Duration::from_secs(10);
        let status = loop {
            let status = status_promptly(&h).await;
            if status["defaults"] == "applied" && status["uploader"]["pending"].is_null() {
                break status;
            }
            assert!(Instant::now() < deadline, "{mode}: {status}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        };
        assert_eq!(status["sharing_mode"], mode, "{status}");
        assert_eq!(*probe.sharing.lock().unwrap(), 0, "{mode} was changed");
        h.stop().await;
    }
}

#[tokio::test]
async fn a_status_read_that_started_before_a_sharing_change_keeps_the_new_mode() {
    let (release, held) = tokio::sync::watch::channel(true);
    let probe = Arc::new(HeldStatusProbe {
        release: held,
        mode: Mutex::new("all"),
        reads: Mutex::new(0),
        sharing: Mutex::new(0),
        answered: Arc::default(),
    });
    let h =
        Harness::start_complete(false, false, Some(probe.clone() as Arc<dyn ProbeRunner>)).await;
    eventually("the account signed in", || {
        h.core.status().account.state == SignInState::SignedIn
    })
    .await;
    // Let the bootstrap's read answer, so only the refresh read below is
    // held. The bootstrap holds the sharing lock across its read, so a
    // sharing change could not run alongside it.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let status = status_promptly(&h).await;
        if status["defaults"] == "applied" {
            break;
        }
        assert!(Instant::now() < deadline, "{status}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // A first sharing change, with no read in flight: it patches the
    // cache and makes the uploader's snapshot stale, so the next status
    // read goes back to the uploader instead of the snapshot.
    let changed = h
        .call(
            IN_CLAUDE,
            "POST",
            "/setup/sharing",
            json!({"mode": "selected"}),
        )
        .await
        .unwrap();
    assert_eq!(changed, json!({"sharing_mode": "selected"}));
    release.send(false).unwrap();
    probe
        .answered
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let reads_before = *probe.reads.lock().unwrap();
    status_promptly(&h).await;
    eventually("the held uploader read", || {
        *probe.reads.lock().unwrap() > reads_before
    })
    .await;
    // A second change lands while that read is still held.
    let changed = h
        .call(IN_CLAUDE, "POST", "/setup/sharing", json!({"mode": "all"}))
        .await
        .unwrap();
    assert_eq!(changed, json!({"sharing_mode": "all"}));
    let reads_held = *probe.reads.lock().unwrap();
    release.send(true).unwrap();
    eventually("the held uploader read answered", || {
        probe.answered.load(std::sync::atomic::Ordering::SeqCst)
    })
    .await;
    // Hold the next reads, so none can land and repair the cache before
    // it is reported: a new read starting proves the held one has been
    // stored (or dropped), because the refresh that starts it runs only
    // after the held task ends.
    release.send(false).unwrap();
    status_promptly(&h).await;
    eventually("the held answer was stored", || {
        *probe.reads.lock().unwrap() > reads_held
    })
    .await;
    let status = status_promptly(&h).await;
    release.send(true).unwrap();
    assert_eq!(status["sharing_mode"], "all", "{status}");
    // Nor may the stale read wipe the cache to a failed read: the uploader
    // is still the running one the earlier reads reported.
    assert_eq!(status["uploader"]["running"], true, "{status}");
    h.stop().await;
}

/// The desktop's account turns automatic activation on for a fresh core,
/// as the desktop's core always did, keeps a person's explicit Off, and a
/// core with no account (the relay CLI's) leaves it unset.
#[tokio::test]
async fn a_desktop_core_starts_with_automatic_activation_on() {
    let probe = || {
        Arc::new(StatusProbe {
            status: Mutex::new(json!({"running": true, "paused": false, "sharing_mode": "selected"})),
            installs: stalled_install(),
        }) as Arc<dyn ProbeRunner>
    };
    let h = Harness::start_inner(true, false, Some(probe()), false).await;
    assert_eq!(h.core.settings()["auto_activate"], json!(true));
    h.stop().await;

    let h = Harness::start_complete(true, false, Some(probe())).await;
    assert_eq!(h.core.settings()["auto_activate"], json!(false), "an explicit Off is kept");
    h.stop().await;

    let h = Harness::start_unchosen().await;
    assert_eq!(h.core.store.read(|store| store.auto_activate), None);
    h.stop().await;
}
