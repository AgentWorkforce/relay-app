//! Feed bodies over their budget: refused as they arrive, by the bytes
//! received, as on the Mac (CloudFeedUpdaterTests).

use std::io::{self, Read};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::*;

#[test]
fn an_asset_past_its_signed_size_is_refused_on_both_paths() {
    let stream = format!("{FEED}/assets/a_01JD3Q4W03");
    let oversized = vec![b'a'; 256 * 1024];
    let signed = |request: &FeedRequest| match (request.method, request.url.as_str()) {
        ("GET", url) if url.starts_with(SIGNED) => ok(oversized.clone()),
        _ => site(request),
    };
    let streaming = |request: &FeedRequest| match (request.method, request.url.as_str()) {
        ("POST", _) => answer(501, "{}"),
        ("GET", url) if url == stream => ok(oversized.clone()),
        _ => site(request),
    };
    for respond in [&signed as &Respond<'_>, &streaming] {
        let (result, requests) = run(respond, |host| archive_bytes(&checked(host), host));
        assert_eq!(result, Err(error(TOO_LARGE)));
        // The manifest and `download-url` answers get the small budget, the
        // download its signed size.
        let budgets: Vec<u64> = requests.iter().map(|request| request.max_body).collect();
        let size = TARBALL.len() as u64;
        assert_eq!(budgets, [MAX_FEED_ANSWER, MAX_FEED_ANSWER, size]);
    }
}

#[test]
fn one_byte_past_the_signed_size_is_refused_as_too_large() {
    let longer = [TARBALL, b"!"].concat();
    let serve = |request: &FeedRequest| match (request.method, request.url.as_str()) {
        ("GET", url) if url.starts_with(SIGNED) => ok(longer.clone()),
        _ => site(request),
    };
    let (result, _) = run(&serve, |host| archive_bytes(&checked(host), host));
    assert_eq!(result, Err(error(TOO_LARGE)));
}

#[test]
fn an_oversized_manifest_or_download_url_answer_is_refused() {
    let oversized = vec![b' '; MAX_FEED_ANSWER as usize + 1];
    let manifest = |request: &FeedRequest| match (request.method, request.url.as_str()) {
        ("GET", STABLE) => ok(oversized.clone()),
        _ => site(request),
    };
    let (result, _) = run(&manifest, check);
    assert_eq!(result, Err(error(TOO_LARGE)));
    let signing = |request: &FeedRequest| match request.method {
        "POST" => ok(oversized.clone()),
        _ => site(request),
    };
    let (result, _) = run(&signing, |host| archive_bytes(&checked(host), host));
    assert_eq!(result, Err(error(TOO_LARGE)));
}

/// An endless body that counts the bytes taken from it.
struct Endless(Arc<AtomicU64>);

impl Read for Endless {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        buffer.fill(b'a');
        self.0.fetch_add(buffer.len() as u64, Ordering::SeqCst);
        Ok(buffer.len())
    }
}

#[test]
fn a_body_is_read_no_further_than_its_budget() {
    let deadline = REFUSAL_BODY_DEADLINE;
    let taken = Arc::new(AtomicU64::new(0));
    assert_eq!(
        answer_body(200, Endless(taken.clone()), 4096, deadline),
        Err(FetchError::TooLarge)
    );
    assert_eq!(taken.load(Ordering::SeqCst), 4097);
    // A `403` keeps its first 64 KiB, for `not_entitled`.
    let taken = Arc::new(AtomicU64::new(0));
    let kept = answer_body(403, Endless(taken.clone()), 4096, deadline).expect("refusal body");
    assert_eq!(kept.len(), 64 * 1024);
    assert_eq!(taken.load(Ordering::SeqCst), 64 * 1024);
    let small = io::Cursor::new(b"small".to_vec());
    assert_eq!(answer_body(200, small, 5, deadline), Ok(b"small".to_vec()));
}

#[test]
fn a_refusal_whose_body_no_caller_reads_is_not_read() {
    for status in [401, 404, 429, 500, 501, 503] {
        let taken = Arc::new(AtomicU64::new(0));
        let body = answer_body(status, Endless(taken.clone()), 4096, REFUSAL_BODY_DEADLINE);
        assert_eq!(body, Ok(Vec::new()), "{status}");
        assert_eq!(taken.load(Ordering::SeqCst), 0, "{status}");
    }
}
