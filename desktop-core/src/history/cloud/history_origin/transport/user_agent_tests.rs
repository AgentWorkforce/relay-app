use super::tests::config;
use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::{Duration, Instant};

/// The request head `agent` sends to a local listener.
fn request_head(agent: ureq::Agent) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let handle = thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut tcp = loop {
            match listener.accept() {
                Ok((tcp, _)) => break tcp,
                Err(e)
                    if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline =>
                {
                    thread::sleep(Duration::from_millis(5));
                }
                error => panic!("expected a connection: {error:?}"),
            }
        };
        // macOS inherits O_NONBLOCK from the listening socket.
        tcp.set_nonblocking(false).unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
        let mut bytes = Vec::new();
        while !bytes.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            tcp.read_exact(&mut byte).unwrap();
            bytes.push(byte[0]);
            assert!(bytes.len() < 8192);
        }
        tcp.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .unwrap();
        String::from_utf8(bytes).unwrap()
    });
    let url = format!("http://{address}/history");
    assert_eq!(agent.get(&url).call().unwrap().status(), 200);
    handle.join().unwrap()
}

fn configure(builder: ureq::AgentBuilder) -> ureq::AgentBuilder {
    builder.redirects(0).timeout(Duration::from_secs(3))
}

fn listed(_: &str, _: u16) -> bool {
    true
}

#[test]
fn the_user_agent_names_the_desktop_release_and_os() {
    assert_eq!(
        user_agent_for("2026.11.12-beta.1", "windows"),
        "agent-relay-desktop/2026.11.12-beta.1 windows"
    );
    let os = crate::telemetry::platform();
    assert!(["macos", "linux", "windows"].contains(&os));
    #[cfg(target_os = "macos")]
    assert_eq!(os, "macos");
    #[cfg(windows)]
    assert_eq!(os, "windows");
    #[cfg(target_os = "linux")]
    assert_eq!(os, "linux");
    assert_eq!(
        user_agent(),
        format!("agent-relay-desktop/{} {os}", agent_relay_node::VERSION)
    );
}

#[test]
fn every_history_agent_sends_the_desktop_user_agent() {
    let expected = format!("\r\nuser-agent: {}\r\n", user_agent()).to_ascii_lowercase();
    let pool = HistoryAgentPool::with_loader(|| config(true), configure);
    let storage = HistoryAgentPool {
        listed,
        ..HistoryAgentPool::with_loader(|| config(true), configure)
    };
    for (name, agent) in [
        (
            "history agent",
            configure(platform_agent(|| config(true))).build(),
        ),
        ("pool", pool.get()),
        ("storage pool", storage.get()),
    ] {
        let head = request_head(agent);
        assert!(
            head.to_ascii_lowercase().contains(&expected),
            "{name} sent {head:?}"
        );
        assert_eq!(head.matches("agent-relay-desktop/").count(), 1, "{name}");
    }
}
