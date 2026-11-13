use super::*;
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::thread;
use std::time::{Duration, Instant};

const CA: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/telemetry/posthog/https/testdata/ca.der"
));
const CERT: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/telemetry/posthog/https/testdata/localhost.der"
));
const KEY: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/src/telemetry/posthog/https/testdata/localhost-key.der"
));

pub(super) fn config(trust_test_ca: bool) -> Arc<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    if trust_test_ca {
        roots.add(CertificateDer::from(CA.to_vec())).unwrap();
    }
    Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    )
}

pub(super) fn server() -> (SocketAddr, thread::JoinHandle<bool>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let end = Instant::now() + Duration::from_secs(5);
        let tcp = loop {
            match listener.accept() {
                Ok((tcp, _)) => break tcp,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < end => {
                    thread::sleep(Duration::from_millis(5));
                }
                _ => return false,
            }
        };
        // macOS inherits O_NONBLOCK from the listening socket.
        tcp.set_nonblocking(false).unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        tcp.set_write_timeout(Some(Duration::from_secs(2))).unwrap();
        let tls = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(CERT.to_vec())],
                PrivatePkcs8KeyDer::from(KEY.to_vec()).into(),
            )
            .unwrap();
        let mut stream =
            rustls::StreamOwned::new(rustls::ServerConnection::new(Arc::new(tls)).unwrap(), tcp);
        let mut request = Vec::new();
        loop {
            let mut part = [0; 1024];
            match stream.read(&mut part) {
                Ok(0) | Err(_) => return false,
                Ok(n) => request.extend_from_slice(&part[..n]),
            }
            assert!(request.len() <= 8192);
            if request.windows(4).any(|p| p == b"\r\n\r\n") {
                break;
            }
        }
        let request = String::from_utf8(request).unwrap();
        assert!(request.starts_with("GET /history HTTP/1.1"));
        assert!(request.contains("Bearer synthetic-history-test"));
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .unwrap();
        true
    });
    (address, handle)
}

#[test]
fn history_transport_uses_the_loaded_roots_and_verifies_the_hostname() {
    // Exercise the actual builder shared with agent(), not an unrelated client.
    for (trust, host, accepted) in [
        (true, "localhost", true),
        (false, "localhost", false),
        (true, "127.0.0.1", false),
    ] {
        let (address, handle) = server();
        let load: RootLoader = if trust {
            || config(true)
        } else {
            || config(false)
        };
        let result = platform_agent(load)
            .redirects(0)
            .timeout(Duration::from_secs(3))
            .build()
            .get(&format!("https://{host}:{}/history", address.port()))
            .set("Authorization", "Bearer synthetic-history-test")
            .call();
        assert_eq!(
            result.is_ok(),
            accepted,
            "trust={trust} host={host}: {result:?}"
        );
        assert_eq!(
            handle.join().unwrap(),
            accepted,
            "only verified TLS may receive the bearer"
        );
    }
}

#[test]
fn override_child() {
    let Ok(mode) = std::env::var("RELAY_HISTORY_TLS_CHILD") else {
        return;
    };
    let key = std::env::var("RELAY_HISTORY_TLS_KEY").unwrap();
    if matches!(mode.as_str(), "before" | "cleared") {
        std::env::set_var(&key, "unused-test-root-path");
    } else if mode == "empty" {
        std::env::set_var(&key, "");
    }
    let loaded = std::sync::atomic::AtomicBool::new(false);
    let builder = with_roots(|| {
        loaded.store(true, std::sync::atomic::Ordering::Relaxed);
        config(false)
    });
    static POOL_LOADS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    fn pool_roots() -> Arc<rustls::ClientConfig> {
        POOL_LOADS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        config(false)
    }
    let pool = HistoryAgentPool::with_loader(pool_roots, |builder| {
        builder.redirects(0).timeout(Duration::from_millis(100))
    });
    assert_eq!(
        loaded.load(std::sync::atomic::Ordering::Relaxed),
        !matches!(mode.as_str(), "before" | "cleared")
    );
    if mode == "after" {
        std::env::set_var(&key, "unused-test-root-path");
    }
    if mode == "cleared" {
        std::env::remove_var(&key);
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let agent = builder
        .redirects(0)
        .timeout(Duration::from_millis(100))
        .build();
    for _ in 0..3 {
        let result = agent
            .get(&format!("http://{}/", listener.local_addr().unwrap()))
            .set("Authorization", "Bearer synthetic-history-test")
            .call();
        assert!(result.is_err()); // The admitted control connects, then times out.
        if matches!(mode.as_str(), "positive" | "empty") {
            assert!(
                listener.accept().is_ok(),
                "ordinary loopback HTTP must connect"
            );
        } else {
            let refused = pool
                .get()
                .get(&format!("http://{}/", listener.local_addr().unwrap()))
                .set("Authorization", "Bearer synthetic-history-test")
                .call();
            assert!(
                matches!(refused, Err(ureq::Error::Transport(ref error)) if certificate_override_refused(error))
            );
            assert_eq!(POOL_LOADS.load(std::sync::atomic::Ordering::Relaxed), 0);
            assert_eq!(
                listener.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
        }
    }
}

#[test]
fn certificate_overrides_refuse_before_or_after_construction_even_when_cleared() {
    for key in ["SSL_CERT_FILE", "SSL_CERT_DIR"] {
        for mode in ["before", "after", "cleared", "positive", "empty"] {
            let out = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "history::cloud::history_origin::transport::tests::override_child",
                ])
                .env_remove("SSL_CERT_FILE")
                .env_remove("SSL_CERT_DIR")
                .env("RELAY_HISTORY_TLS_CHILD", mode)
                .env("RELAY_HISTORY_TLS_KEY", key)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{key} {mode}: {}",
                String::from_utf8_lossy(&out.stdout)
            );
            assert!(String::from_utf8_lossy(&out.stdout).contains("1 passed"));
            let diagnostic = String::from_utf8_lossy(&out.stderr);
            if matches!(mode, "before" | "after" | "cleared") {
                assert_eq!(
                    diagnostic
                        .matches("History refuses certificate-file overrides")
                        .count(),
                    1
                );
                assert!(
                    diagnostic.contains("SSL_CERT_FILE") && diagnostic.contains("SSL_CERT_DIR")
                );
                assert!(!diagnostic.contains("unused-test-root-path"));
            } else {
                assert!(!diagnostic.contains("History refuses certificate-file overrides"));
            }
        }
    }
}

#[test]
fn kind_platform_history_trust_smoke_when_requested() {
    if std::env::var("RELAY_HISTORY_KIND_PLATFORM_TLS_CHECK").as_deref() != Ok("1") {
        return;
    }
    assert!(roots_are_safe());
    let url = "https://relay.kind.agentrelay.com/login";
    assert_eq!(
        agent()
            .redirects(0)
            .timeout(Duration::from_secs(3))
            .build()
            .get(url)
            .call()
            .unwrap()
            .status(),
        200
    );
    let old = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout(Duration::from_secs(3))
        .build()
        .get(url)
        .call();
    match old {
        Err(ureq::Error::Transport(e)) => assert!(e.to_string().contains("UnknownIssuer")),
        _ => panic!("the old bundled-root transport must refuse the private CA"),
    }
}

#[test]
fn the_default_history_agent_installs_the_platform_root_loader() {
    LOADER.with(|slot| slot.set(None));
    let _builder = agent();
    let load = LOADER
        .with(|slot| slot.take())
        .expect("agent must use the injected platform path");
    assert!(std::ptr::fn_addr_eq(load, client_config as RootLoader));
}
