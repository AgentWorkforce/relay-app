use super::*;

/// A site build's storage origin, as a storage pool would list it.
fn corp_store(host: &str, port: u16) -> bool {
    host.eq_ignore_ascii_case("minio.corp.example") && port == 9000
}

fn at(ip: &str, port: u16) -> Vec<SocketAddr> {
    vec![SocketAddr::new(ip.parse().unwrap(), port)]
}

/// Holds the development flag unset, which would exempt every address.
fn without_development_flag<T>(test: impl FnOnce() -> T) -> T {
    let _guard = crate::history::cloud::tests::ENV_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let previous = std::env::var_os("RELAYHISTORY_ALLOW_UNTRUSTED_CLOUD_BASE_URL");
    std::env::remove_var("RELAYHISTORY_ALLOW_UNTRUSTED_CLOUD_BASE_URL");
    let result = test();
    if let Some(value) = previous {
        std::env::set_var("RELAYHISTORY_ALLOW_UNTRUSTED_CLOUD_BASE_URL", value);
    }
    result
}

#[test]
fn a_listed_storage_host_and_port_may_resolve_to_a_private_address() {
    without_development_flag(|| {
        for private in ["10.0.0.5", "192.168.1.20", "fd12:3456::1"] {
            assert!(
                admit("minio.corp.example:9000", at(private, 9000), corp_store).is_ok(),
                "{private}"
            );
        }
    });
}

#[test]
fn only_the_exact_listed_host_and_port_skip_the_address_rule() {
    without_development_flag(|| {
        for (netloc, listed) in [
            // The same host on another port.
            ("minio.corp.example:9001", corp_store as StorageListed),
            ("minio.corp.example:443", corp_store),
            // Another host, a lookalike included.
            ("other.corp.example:9000", corp_store),
            ("minio.corp.example.evil.example:9000", corp_store),
            ("evil-minio.corp.example:9000", corp_store),
            // The History and Cloud agents list nothing.
            ("minio.corp.example:9000", no_storage),
        ] {
            let refused = admit(netloc, at("10.0.0.5", 9000), listed).unwrap_err();
            assert_eq!(
                refused.kind(),
                std::io::ErrorKind::PermissionDenied,
                "{netloc}"
            );
        }
        // A public address passes either way, as before.
        assert!(admit("other.corp.example:9000", at("1.1.1.1", 9000), no_storage).is_ok());
    });
}

#[test]
fn only_a_storage_pool_lists_storage_origins() {
    let configure: fn(ureq::AgentBuilder) -> ureq::AgentBuilder = |builder| builder.redirects(0);
    let history = HistoryAgentPool::new(configure);
    assert!(std::ptr::fn_addr_eq(
        history.listed,
        no_storage as StorageListed
    ));
    let storage = HistoryAgentPool::storage(configure, corp_store);
    assert!(std::ptr::fn_addr_eq(
        storage.listed,
        corp_store as StorageListed
    ));
    without_development_flag(|| {
        // The shared History rule is unchanged for the same host.
        assert!(connectable("minio.corp.example", at("10.0.0.5", 9000)).is_err());
    });
}
