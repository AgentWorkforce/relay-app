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
