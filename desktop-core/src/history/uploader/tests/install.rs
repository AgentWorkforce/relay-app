//! Real child processes reproduce a collector inheriting setup's output pipes
//! on Windows as well as Unix. No installed core or account is involved.
use super::*;

const HELPER: &str = "history::uploader::tests::install::inherited_pipe_helper";
const ROLE: &str = "RELAY_TEST_INSTALL_PIPE_ROLE";
const DIRECTORY: &str = "RELAY_TEST_INSTALL_PIPE_DIRECTORY";

fn helper_args() -> Vec<String> {
    ["--exact", HELPER, "--ignored", "--nocapture"]
        .map(str::to_owned)
        .to_vec()
}

// Release the pipe holder even when an assertion fails, without terminating
// any real service. The helper also has its own deadline as a final backstop.
struct ReleaseHolder(PathBuf);

impl Drop for ReleaseHolder {
    fn drop(&mut self) {
        let _ = std::fs::write(self.0.join("release"), b"");
        // Keep the directory alive until the holder observes the release.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !self.0.join("holder-done").exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[test]
#[ignore = "subprocess fixture, launched only by the install regression tests"]
fn inherited_pipe_helper() {
    use std::io::Write;

    let role = std::env::var(ROLE).expect("fixture role");
    let directory = PathBuf::from(std::env::var_os(DIRECTORY).expect("fixture directory"));
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    if role == "holder" {
        std::fs::write(directory.join("holder-ready"), b"").unwrap();
        while !directory.join("release").exists() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        std::fs::write(directory.join("holder-done"), b"").unwrap();
        return;
    }

    let mut holder = std::process::Command::new(std::env::current_exe().unwrap());
    holder
        .args(helper_args())
        .env(ROLE, "holder")
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    agent_relay_node::platform::process::hide_window(&mut holder);
    let _holder = holder.spawn().unwrap();
    while !directory.join("holder-ready").exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "pipe holder did not start"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    // The leading newline separates the first event from libtest's prefix.
    println!("\n{}", json!({"event": "connected"}));
    println!("{}", json!({"event": "ready"}));
    std::io::stdout().flush().unwrap();
    if role == "cancel" {
        while std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    if role == "failure" {
        eprintln!("Fixture setup failed.");
        std::process::exit(1);
    }
    std::process::exit(0);
}

async fn install_with_inherited_pipes(role: &str) {
    let directory = tempfile::tempdir().unwrap();
    let _release = ReleaseHolder(directory.path().to_path_buf());
    let runner = ProcessRunner::new(std::env::current_exe().unwrap())
        .with_env(ROLE, role)
        .with_env(DIRECTORY, directory.path());
    let mut run = runner.install(helper_args(), None).unwrap();
    for event in ["connected", "ready"] {
        let received = tokio::time::timeout(Duration::from_secs(5), run.events.recv())
            .await
            .expect("setup must emit its events")
            .expect("event stream ended early");
        assert_eq!(received["event"], event);
    }
    if role == "cancel" {
        (run.cancel)();
    }
    // Account::connect drains this channel before awaiting finished. A
    // detached reader would leave it waiting for the pipe holder forever.
    assert!(
        tokio::time::timeout(Duration::from_secs(5), run.events.recv())
            .await
            .expect("setup's event stream must close while the descendant is alive")
            .is_none()
    );
    assert!(!directory.path().join("release").exists());
    assert!(!directory.path().join("holder-done").exists());
    let outcome = tokio::time::timeout(Duration::from_secs(5), run.finished)
        .await
        .expect("setup outcome must follow its events")
        .unwrap();
    match role {
        "success" => assert_eq!(outcome, Ok(())),
        "failure" => assert_eq!(outcome.unwrap_err().message, "Fixture setup failed."),
        "cancel" => assert!(outcome.unwrap_err().cancelled),
        _ => unreachable!(),
    }
    assert!(runner.children.lock().unwrap().is_empty());
}

#[tokio::test]
async fn successful_install_closes_events_with_inherited_pipes() {
    install_with_inherited_pipes("success").await;
}

#[tokio::test]
async fn failed_install_closes_events_with_inherited_pipes() {
    install_with_inherited_pipes("failure").await;
}

#[tokio::test]
async fn cancelled_install_closes_events_with_inherited_pipes() {
    install_with_inherited_pipes("cancel").await;
}
