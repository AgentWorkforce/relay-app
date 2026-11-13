use super::*;
use crate::history::destination::{failure_of, http_error, verdict};
use std::net::TcpListener;
use std::time::Duration;

#[test]
fn override_classification_child() {
    let Ok(mode) = std::env::var("RELAY_HISTORY_CLASSIFICATION_CHILD") else {
        return;
    };
    let key = std::env::var("RELAY_HISTORY_CLASSIFICATION_KEY").unwrap();
    if matches!(mode.as_str(), "before" | "cleared") {
        std::env::set_var(&key, "unused-test-root-path");
    }
    let agent = cloud::history_agent()
        .redirects(0)
        .timeout(Duration::from_millis(100))
        .build();
    if mode == "after" {
        std::env::set_var(&key, "unused-test-root-path");
    }
    if mode == "cleared" {
        std::env::remove_var(&key);
    }
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    for attempt in 0..3 {
        let raw = agent.get(&url).call().unwrap_err();
        let error = if attempt == 1 {
            super::super::guard::retry_unresolved(cloud::RefreshUnavailable::wrap(raw.into()))
        } else {
            http_error(raw, "")
        };
        let expected = DeliveryFailure::Transient;
        assert_eq!(failure_of(&error), Some(expected));
        let verdict = verdict(&error);
        assert_eq!(verdict.failure, expected);
        // Never a receiver Retry-After: Retry must be able to wake the wait.
        assert_eq!(verdict.retry_after_ms, None);
        if mode == "positive" {
            assert_eq!(verdict.local_wait_ms, None);
            assert!(listener.accept().is_ok());
        } else {
            assert!(expected.retryable());
            assert_eq!(verdict.local_wait_ms, Some(CONFIGURATION_RETRY_MS));
            assert_eq!(
                listener.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
        }
    }
    if matches!(mode.as_str(), "before" | "after") {
        let auth = cloud::StoredAuth {
            base_url: url,
            ..Default::default()
        };
        let error = cloud::send_with_auth_refresh_checked_with_budget(
            &auth,
            |_, _| panic!("configuration refusal must happen before auth send"),
            |e| e.into(),
            |_| Ok(()),
            None,
        )
        .unwrap_err();
        assert!(error.is::<cloud::HistoryTlsConfigurationRefused>());
        assert_eq!(failure_of(&error), Some(DeliveryFailure::Transient));
        assert_eq!(verdict(&error).failure, DeliveryFailure::Transient);
        assert_eq!(verdict(&error).retry_after_ms, None);
        assert_eq!(verdict(&error).local_wait_ms, Some(CONFIGURATION_RETRY_MS));
    }
}

#[test]
fn configuration_refusal_defers_before_auth_and_on_a_resolver_race() {
    for key in ["SSL_CERT_FILE", "SSL_CERT_DIR"] {
        for mode in ["before", "after", "cleared", "positive"] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "history::destination::tls::tests::override_classification_child",
                ])
                .env_remove("SSL_CERT_FILE")
                .env_remove("SSL_CERT_DIR")
                .env("RELAY_HISTORY_CLASSIFICATION_CHILD", mode)
                .env("RELAY_HISTORY_CLASSIFICATION_KEY", key)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{key} {mode}: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(!String::from_utf8_lossy(&output.stderr).contains("unused-test-root-path"));
        }
    }
}
