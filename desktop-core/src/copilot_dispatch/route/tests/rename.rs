use super::*;

#[tokio::test]
async fn a_copilot_rename_after_permission_revokes_the_old_sender_before_http() {
    for after_key_resolution in [false, true] {
        let f = fixture().await;
        let (ticket, call) = pending(&f).await;
        control(&f, &ticket, &call, true);
        let queued = dispatch(&f, ticket, call);
        if !after_key_resolution {
            crate::attestation_route::tests::copilot::rename::rename(&f);
        }
        let reply = queued.run_after_resolve(
            |_, _, _, _| panic!("renamed pending call reached HTTP"),
            |_| {
                if after_key_resolution {
                    crate::attestation_route::tests::copilot::rename::rename(&f);
                }
            },
        );
        assert_eq!(reply["error"]["code"], -32000);
        assert_eq!(reply["error"]["data"]["class"], "owner_changed");
    }
}

#[tokio::test]
async fn a_copilot_sender_removed_from_the_accepted_snapshot_cannot_post() {
    let f = fixture().await;
    let (ticket, call) = pending(&f).await;
    control(&f, &ticket, &call, true);
    f.core
        .store
        .update(|store| {
            store
                .registrations
                .iter_mut()
                .find(|r| r.target.kind == SessionKind::Copilot)
                .unwrap()
                .missing_since = Some("2026-11-17T00:00:00Z".into());
        })
        .unwrap();
    Identities::new(f.core.clone())
        .sync_desktop_identities()
        .await;
    f.core
        .store
        .update(|store| {
            store
                .registrations
                .iter_mut()
                .find(|r| r.target.kind == SessionKind::Copilot)
                .unwrap()
                .missing_since = None;
        })
        .unwrap();
    let reply = dispatch(&f, ticket, call)
        .run_with(|_, _, _, _| panic!("an unsynced reactivated sender reached HTTP"));
    assert_eq!(reply["error"]["data"]["class"], "owner_changed");
}
