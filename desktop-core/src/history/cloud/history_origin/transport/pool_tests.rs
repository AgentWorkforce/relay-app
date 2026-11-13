use super::tests::{config, server};
use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::{Duration, Instant};

thread_local! {
    static CURRENT: std::cell::RefCell<Option<Arc<rustls::ClientConfig>>> = const { std::cell::RefCell::new(None) };
}

fn roots() -> Arc<rustls::ClientConfig> {
    CURRENT.with(|slot| slot.borrow().as_ref().unwrap().clone())
}

fn configure(builder: ureq::AgentBuilder) -> ureq::AgentBuilder {
    builder.redirects(0).timeout(Duration::from_secs(3))
}

#[test]
fn a_pool_reuses_connections_until_the_trust_snapshot_changes() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let mut retained = Vec::new();
        for requests in [2, 1] {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut tcp = loop {
                match listener.accept() {
                    Ok((tcp, _)) => break tcp,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(5));
                    }
                    error => return Err(format!("expected a connection: {error:?}")),
                }
            };
            tcp.set_nonblocking(false).unwrap();
            tcp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            for _ in 0..requests {
                let mut bytes = Vec::new();
                while !bytes.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    tcp.read_exact(&mut byte).unwrap();
                    bytes.push(byte[0]);
                    assert!(bytes.len() < 8192);
                }
                tcp.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                    .unwrap();
            }
            // Keep the first socket open: the third request must use a new
            // pool, not merely reconnect after the server closed a socket.
            retained.push(tcp);
        }
        Ok(())
    });
    CURRENT.with(|slot| *slot.borrow_mut() = Some(config(true)));
    let pool = HistoryAgentPool::with_loader(roots, configure);
    let url = format!("http://{address}/history");
    for _ in 0..2 {
        assert_eq!(pool.get().get(&url).call().unwrap().status(), 200);
    }
    CURRENT.with(|slot| *slot.borrow_mut() = Some(config(true)));
    assert_eq!(pool.get().get(&url).call().unwrap().status(), 200);
    assert_eq!(handle.join().unwrap(), Ok(()));
    CURRENT.with(|slot| *slot.borrow_mut() = None);
}

#[test]
fn a_replaced_snapshot_changes_real_tls_admission_without_leaking_the_bearer() {
    let pool = HistoryAgentPool::with_loader(roots, configure);
    for trusted in [true, false, true] {
        CURRENT.with(|slot| *slot.borrow_mut() = Some(config(trusted)));
        let (address, handle) = server();
        let result = pool
            .get()
            .get(&format!("https://localhost:{}/history", address.port()))
            .set("Authorization", "Bearer synthetic-history-test")
            .call();
        assert_eq!(result.is_ok(), trusted, "{result:?}");
        assert_eq!(handle.join().unwrap(), trusted);
    }
    CURRENT.with(|slot| *slot.borrow_mut() = None);
}
