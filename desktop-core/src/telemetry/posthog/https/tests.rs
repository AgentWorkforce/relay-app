use super::*;
use serde_json::json;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::thread;
use std::time::Duration;

fn trusted() -> Arc<rustls::ClientConfig> {
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(rustls::pki_types::CertificateDer::from(
            include_bytes!("testdata/ca.der").to_vec(),
        ))
        .unwrap();
    tls(roots)
}

fn tls(roots: rustls::RootCertStore) -> Arc<rustls::ClientConfig> {
    Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    )
}

// Read the complete test request before closing the socket. Answering after
// only the headers can reset a connection whose JSON body is still in flight.
fn read_request(stream: &mut impl Read) -> Option<String> {
    let mut bytes = Vec::new();
    loop {
        let mut chunk = [0; 4096];
        let count = stream.read(&mut chunk).ok()?;
        if count == 0 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..count]);
        assert!(bytes.len() <= 8192);
        if let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&bytes[..end]);
            let length = headers.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse::<usize>().unwrap())
            })?;
            if bytes.len() >= end + 4 + length {
                return Some(String::from_utf8(bytes).unwrap());
            }
        }
    }
}

fn server(response: String) -> (SocketAddr, thread::JoinHandle<bool>) {
    let config = Arc::new(
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![rustls::pki_types::CertificateDer::from(
                    include_bytes!("testdata/localhost.der").to_vec(),
                )],
                rustls::pki_types::PrivatePkcs8KeyDer::from(
                    include_bytes!("testdata/localhost-key.der").to_vec(),
                )
                .into(),
            )
            .unwrap(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let (tcp, _) = listener.accept().unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        tcp.set_write_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut stream =
            rustls::StreamOwned::new(rustls::ServerConnection::new(config).unwrap(), tcp);
        let Some(request) = read_request(&mut stream) else {
            return false;
        };
        assert!(request.starts_with("POST /events HTTP/1.1"));
        assert!(request.contains("Bearer synthetic-analytics-test"));
        stream.write_all(response.as_bytes()).unwrap();
        true
    });
    (address, handle)
}

fn send(tls: Arc<rustls::ClientConfig>, host: &str, address: SocketAddr) -> Result<Answer, String> {
    CURRENT_ROOTS.with(|slot| *slot.borrow_mut() = Some(tls));
    let result = Https::with_roots(current_roots).post(
        &format!("https://{host}:{}/events", address.port()),
        Some("synthetic-analytics-test"),
        json!({"synthetic_test":true}),
    );
    CURRENT_ROOTS.with(|slot| *slot.borrow_mut() = None);
    result
}

#[test]
fn a_verified_matching_host_receives_the_request_but_no_other_tls_peer_does() {
    let (address, handle) = server(
        "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nRetry-After: 7\r\nConnection: close\r\n\r\n"
            .into(),
    );
    assert_eq!(
        send(trusted(), "localhost", address).unwrap(),
        Answer {
            status: 202,
            retry_after: Some("7".into())
        },
    );
    assert!(handle.join().unwrap());

    let (address, handle) = server(String::new());
    assert!(send(trusted(), "127.0.0.1", address).is_err());
    assert!(
        !handle.join().unwrap(),
        "wrong host must receive no HTTP bearer"
    );

    let (address, handle) = server(String::new());
    assert!(send(tls(rustls::RootCertStore::empty()), "localhost", address).is_err());
    assert!(
        !handle.join().unwrap(),
        "untrusted CA must receive no HTTP bearer"
    );
}

#[test]
fn a_redirect_never_forwards_the_bearer() {
    let target = TcpListener::bind("127.0.0.1:0").unwrap();
    target.set_nonblocking(true).unwrap();
    let (address, handle) = server(format!(
        "HTTP/1.1 307 Temporary Redirect\r\nLocation: https://{}/elsewhere\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        target.local_addr().unwrap(),
    ));
    assert_eq!(send(trusted(), "localhost", address).unwrap().status, 307);
    assert!(handle.join().unwrap());
    assert_eq!(
        target.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn environment_refusal_child() {
    let Ok(mode) = std::env::var("RELAY_ANALYTICS_TLS_CHILD") else {
        return;
    };
    let key = std::env::var("RELAY_ANALYTICS_TLS_KEY").unwrap();
    if mode == "before" || mode == "cleared" {
        std::env::set_var(&key, "unused-test-root-path");
    } else if mode == "empty" {
        std::env::set_var(&key, "");
    }
    let transport = Https::new();
    if mode == "after" {
        std::env::set_var(&key, "unused-test-root-path");
    }
    if mode == "cleared" {
        std::env::remove_var(&key);
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/events", listener.local_addr().unwrap());
    if mode == "network" {
        drop(listener);
        assert!(matches!(
            crate::telemetry::gateway::post(
                &transport,
                &url,
                "synthetic-test",
                vec![json!({"event": "desktop.app_started"})]
            ),
            crate::telemetry::gateway::Delivery::Retry { after: None, .. }
        ));
        return;
    }
    if mode == "positive" || mode == "empty" {
        let handle = thread::spawn(move || {
            let (mut tcp, _) = listener.accept().unwrap();
            tcp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            assert!(read_request(&mut tcp).is_some());
            tcp.write_all(
                b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            )
            .unwrap();
        });
        assert_eq!(transport.post(&url, None, json!({})).unwrap().status, 202);
        handle.join().unwrap();
    } else {
        listener.set_nonblocking(true).unwrap();
        let error = transport
            .post(&url, Some("synthetic-test"), json!({}))
            .unwrap_err();
        assert_eq!(
            error,
            "Certificate file overrides are unsupported for analytics."
        );
        assert_refusal_drops_batches(
            transport,
            &format!("https://{}", listener.local_addr().unwrap()),
        );
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}

fn assert_refusal_drops_batches(transport: Https, site: &str) {
    use crate::telemetry::posthog::{
        tests::Signed, PostHogProject, PostHogSender, SessionBearer, SessionSlot,
    };
    use crate::telemetry::{gateway::Delivery, TelemetrySender};
    let signed = Arc::new(Signed(std::sync::Mutex::new(Some("synthetic-test"))));
    let slot = SessionSlot::default();
    let weak = Arc::downgrade(&signed) as std::sync::Weak<dyn SessionBearer>;
    assert!(slot.set(weak).is_ok());
    let sender = PostHogSender::with_transport(
        PostHogProject::site_collector(site).unwrap(),
        slot,
        transport,
    );
    for _ in 0..3 {
        assert_eq!(
            sender.deliver(vec![json!({"event": "desktop.app_started"})]),
            Delivery::Dropped("Certificate file overrides are unsupported for analytics.".into())
        );
    }
}

#[test]
fn file_overrides_refuse_before_or_after_construction_even_when_cleared() {
    for key in ["SSL_CERT_FILE", "SSL_CERT_DIR"] {
        for mode in ["before", "after", "cleared", "positive", "empty", "network"] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "telemetry::posthog::https::tests::environment_refusal_child",
                ])
                .env_remove("SSL_CERT_FILE")
                .env_remove("SSL_CERT_DIR")
                .env("RELAY_ANALYTICS_TLS_CHILD", mode)
                .env("RELAY_ANALYTICS_TLS_KEY", key)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{key} {mode}: {}",
                String::from_utf8_lossy(&output.stdout)
            );
            assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
        }
    }
}

/// Explicitly requested only on the owned Kind QA host. No CA is installed,
/// no credential is supplied and the gateway must refuse without an event.
#[test]
fn kind_platform_trust_smoke_when_requested() {
    if std::env::var("RELAY_ANALYTICS_KIND_PLATFORM_TLS_CHECK").as_deref() != Ok("1") {
        return;
    }
    assert!(platform_root_environment_is_safe());
    let url = "https://relay.kind.agentrelay.com/api/v1/analytics/events";
    assert_eq!(Https::new().post(url, None, json!({})).unwrap().status, 401);

    // The old transport cannot verify this private CA. Keep that failure as
    // the causal control for the positive platform-trust request above.
    let old = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout(SEND_TIMEOUT)
        .build()
        .post(url)
        .send_json(json!({}));
    match old {
        Err(ureq::Error::Transport(error)) => {
            assert!(error.to_string().contains("UnknownIssuer"));
        }
        _ => panic!("the old bundled-root transport must refuse the private CA"),
    }
}

#[test]
fn default_analytics_installs_the_platform_root_loader() {
    assert!(std::ptr::fn_addr_eq(
        Https::new().load_roots,
        client_config as RootLoader
    ));
}

thread_local! {
    static CURRENT_ROOTS: std::cell::RefCell<Option<Arc<rustls::ClientConfig>>> = const { std::cell::RefCell::new(None) };
}

fn current_roots() -> Arc<rustls::ClientConfig> {
    CURRENT_ROOTS.with(|slot| slot.borrow().as_ref().unwrap().clone())
}

#[test]
fn consecutive_events_reuse_the_connection_until_the_trust_arc_changes() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let mut requests = 0;
        for expected in [2, 1] {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let (mut tcp, _) = loop {
                match listener.accept() {
                    Ok(connection) => break connection,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            && std::time::Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(e) => panic!("expected a connection: {e}"),
                }
            };
            tcp.set_nonblocking(false).unwrap();
            tcp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            for _ in 0..expected {
                assert!(read_request(&mut tcp).is_some());
                requests += 1;
                tcp.write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n")
                    .unwrap();
            }
        }
        requests
    });
    let roots = trusted();
    CURRENT_ROOTS.with(|slot| *slot.borrow_mut() = Some(roots.clone()));
    let sender = Https::with_roots(current_roots);
    let url = format!("http://{address}/events");
    for _ in 0..2 {
        assert_eq!(sender.post(&url, None, json!({})).unwrap().status, 202);
    }
    assert!(Arc::ptr_eq(
        &sender.pooled.lock().unwrap().as_ref().unwrap().0,
        &roots
    ));
    let replacement = trusted();
    assert!(!Arc::ptr_eq(&roots, &replacement));
    CURRENT_ROOTS.with(|slot| *slot.borrow_mut() = Some(replacement.clone()));
    assert_eq!(sender.post(&url, None, json!({})).unwrap().status, 202);
    assert!(Arc::ptr_eq(
        &sender.pooled.lock().unwrap().as_ref().unwrap().0,
        &replacement
    ));
    assert_eq!(server.join().unwrap(), 3);
    CURRENT_ROOTS.with(|slot| *slot.borrow_mut() = None);
}
