use super::super::storage_origins::{canonical_origin, compiled, parse, OriginsError, KEY};
use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use std::time::Duration;

fn list(origins: &[(&str, u16)]) -> BTreeSet<Origin> {
    origins
        .iter()
        .map(|(host, port)| (host.to_string(), *port))
        .collect()
}

#[test]
fn canonical_origins_are_accepted_and_folded() {
    for (raw, canonical) in [
        ("https://store.example", "https://store.example"),
        ("HTTPS://Store.Example/", "https://store.example"),
        ("https://store.example:443", "https://store.example"),
        (
            "https://minio.corp.example:9000",
            "https://minio.corp.example:9000",
        ),
        (
            " https://s3-1.corp.example:65535 ",
            "https://s3-1.corp.example:65535",
        ),
    ] {
        assert_eq!(canonical_origin(raw).as_deref(), Ok(canonical), "{raw}");
    }
    let folded = parse(
        r#"["https://Store.Example", "https://store.example:443/", "https://minio.corp.example:9000"]"#,
    )
    .unwrap();
    assert_eq!(
        folded.into_iter().collect::<Vec<_>>(),
        ["https://minio.corp.example:9000", "https://store.example"]
    );
    assert_eq!(parse("").unwrap(), BTreeSet::new());
    assert_eq!(parse("  \n").unwrap(), BTreeSet::new());
    assert_eq!(parse("[]").unwrap(), BTreeSet::new());
}

#[test]
fn anything_but_an_https_origin_on_a_dns_name_is_refused() {
    for (raw, refused) in [
        ("http://store.example", OriginsError::NotHttps),
        ("store.example", OriginsError::NotHttps),
        ("ftp://store.example", OriginsError::NotHttps),
        ("https://user@store.example", OriginsError::Credentials),
        ("https://user:pass@store.example", OriginsError::Credentials),
        ("https://store.example/bucket", OriginsError::NotOrigin),
        ("https://store.example//", OriginsError::NotOrigin),
        ("https://store.example?x=1", OriginsError::NotOrigin),
        ("https://store.example/?x", OriginsError::NotOrigin),
        ("https://store.example#top", OriginsError::NotOrigin),
        ("https://10.0.0.5", OriginsError::Host),
        ("https://10.0.0.5:9000", OriginsError::Host),
        ("https://127.1", OriginsError::Host),
        ("https://0x7f000001", OriginsError::Host),
        ("https://store.0x7f", OriginsError::Host),
        ("https://[fd12::1]", OriginsError::Host),
        ("https://[fd12::1]:9000", OriginsError::Host),
        ("https://", OriginsError::Host),
        ("https://store.example.", OriginsError::Host),
        ("https://-store.example", OriginsError::Host),
        ("https://store_1.example", OriginsError::Host),
        ("https://store.example:", OriginsError::Port),
        ("https://store.example:0", OriginsError::Port),
        ("https://store.example:65536", OriginsError::Port),
        ("https://store.example:+90", OriginsError::Port),
        ("https://store.example:9000:1", OriginsError::Host),
    ] {
        assert_eq!(canonical_origin(raw), Err(refused), "{raw}");
    }
    assert_eq!(parse("https://store.example"), Err(OriginsError::NotJson));
    assert_eq!(
        parse(r#""https://store.example""#),
        Err(OriginsError::NotArray)
    );
    assert_eq!(parse("[1]"), Err(OriginsError::NotArray));
    assert_eq!(
        parse(r#"["https://store.example", "http://store.example"]"#),
        Err(OriginsError::NotHttps)
    );
}

#[test]
fn a_list_outside_a_site_build_is_refused_without_its_value() {
    let text = r#"["https://secret-store.corp.example"]"#;
    let refused = compiled(text, false).unwrap_err();
    assert_eq!(refused, OriginsError::NotSiteBuild);
    let message = refused.to_string();
    assert!(message.starts_with(KEY), "{message}");
    assert!(!message.contains("secret-store"), "{message}");
    let bad = canonical_origin("http://secret-store.corp.example").unwrap_err();
    assert!(!bad.to_string().contains("secret-store"));

    assert_eq!(
        compiled(text, true).as_deref(),
        Ok(r#"["https://secret-store.corp.example"]"#)
    );
    assert_eq!(compiled("", false).as_deref(), Ok("[]"));
    assert_eq!(compiled("[]", false).as_deref(), Ok("[]"));
}

#[test]
fn this_build_lists_no_storage_origin() {
    assert_eq!(
        env!("DESKTOP_HISTORY_UPLOAD_STORAGE_ORIGINS_JSON"),
        "[]",
        "the tests build without {KEY}"
    );
    assert!(LISTED.is_empty());
    assert!(!url_listed("https://minio.corp.example:9000/inbox/key"));
}

#[test]
fn what_build_rs_hands_on_reads_back_as_host_and_effective_port() {
    let origins = origins_from_json(
        &compiled(
            r#"["https://store.example", "https://MinIO.corp.example:9000"]"#,
            true,
        )
        .unwrap(),
    );
    assert_eq!(
        origins,
        list(&[("minio.corp.example", 9000), ("store.example", 443)])
    );
    assert!(origins_from_json("not json").is_empty());
}

#[test]
fn a_put_url_is_listed_only_on_the_exact_scheme_host_and_port() {
    let listed = list(&[("minio.corp.example", 9000), ("store.example", 443)]);
    for url in [
        "https://minio.corp.example:9000/inbox/key",
        "https://MINIO.Corp.Example:9000/inbox/key",
        "https://store.example/inbox/key",
        "https://store.example:443/inbox/key",
    ] {
        assert!(url_listed_in(&listed, url), "{url}");
    }
    for url in [
        "http://minio.corp.example:9000/inbox/key",
        "https://minio.corp.example/inbox/key",
        "https://minio.corp.example:9001/inbox/key",
        "http://store.example:443/inbox/key",
        "https://store.example:8443/inbox/key",
        "https://other.corp.example:9000/inbox/key",
        "https://corp.example:9000/inbox/key",
        "https://minio.corp.example.evil.example:9000/inbox/key",
        "https://10.0.0.5:9000/inbox/key",
        "not a url",
    ] {
        assert!(!url_listed_in(&listed, url), "{url}");
    }
    assert!(listed_in(&listed, "MinIO.corp.example", 9000));
    assert!(!listed_in(&listed, "minio.corp.example", 443));
}

#[test]
fn a_listed_url_still_passes_the_put_rule() {
    // `allowed` is unchanged: listing never admits what it refuses.
    assert!(super::super::allowed("https://minio.corp.example:9000/k"));
    assert!(!super::super::allowed(
        "https://user@minio.corp.example:9000/k"
    ));
}

/// A redirect is the storage's answer, never followed: the agent hands the
/// 307 back and the redirect's target sees no connection.
#[test]
fn the_storage_agents_refuse_redirects() {
    let target = TcpListener::bind("127.0.0.1:0").unwrap();
    target.set_nonblocking(true).unwrap();
    let elsewhere = target.local_addr().unwrap();
    for agent in [storage_agent(), fresh_storage_agent()] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let (mut tcp, _) = listener.accept().unwrap();
            tcp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
            let mut bytes = Vec::new();
            while !bytes.ends_with(b"\r\n\r\n") && bytes.len() < 8192 {
                let mut byte = [0];
                tcp.read_exact(&mut byte).unwrap();
                bytes.push(byte[0]);
            }
            // Read the body too: a socket closed with unread bytes sends a
            // reset, and macOS then drops the 307 the client has not read
            // yet and reports the reset instead (Linux still hands it over).
            let head = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
            let length = head
                .lines()
                .find_map(|line| line.strip_prefix("content-length:"))
                .map_or(0, |value| value.trim().parse::<usize>().unwrap());
            let mut body = vec![0; length];
            tcp.read_exact(&mut body).unwrap();
            let answer = format!(
                "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{elsewhere}/k\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            tcp.write_all(answer.as_bytes()).unwrap();
            // Close only after the client has: no reset can overtake the answer.
            tcp.shutdown(std::net::Shutdown::Write).unwrap();
            let _ = tcp.read_to_end(&mut Vec::new());
        });
        let sent = agent
            .put(&format!("http://{address}/k"))
            .timeout(Duration::from_secs(3))
            .send_bytes(b"batch");
        // ureq hands an unfollowed 3xx back as the response itself, which
        // `put` takes as unstored. The response is dropped here, closing the
        // connection the server waits on.
        let status = match sent {
            Ok(response) => Some(response.status()),
            Err(ureq::Error::Status(status, _)) => Some(status),
            Err(_) => None,
        };
        server.join().unwrap();
        assert_eq!(status, Some(307));
        assert_eq!(
            target.accept().map(|_| ()).unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}
