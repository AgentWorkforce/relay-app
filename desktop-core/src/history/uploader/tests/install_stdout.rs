use super::*;
const HELPER: &str = "history::uploader::install_stdout_tests::helper_process";
#[test]
#[ignore = "synthetic subprocess invoked by the install lifecycle regression"]
fn helper_process() {
    let mode = std::env::var("RELAY_INSTALL_PIPE_FIXTURE").unwrap();
    if mode == "hold" {
        std::thread::sleep(Duration::from_secs(6));
        return;
    }
    for event in ["authenticated", "connected", "ready"] {
        println!("{}", json!({"event": event}));
    }
    if mode != "eof" {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", HELPER, "--nocapture"])
            .env("RELAY_INSTALL_PIPE_FIXTURE", "hold")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
    }
    match mode.as_str() {
        "error" => panic!("synthetic helper failed"),
        "cancel" => std::thread::sleep(Duration::from_secs(10)),
        _ => {}
    }
}
#[tokio::test]
async fn install_closes_inherited_stdout_after_success_error_and_cancellation() {
    for mode in ["eof", "success", "error", "cancel"] {
        let runner = ProcessRunner::new(std::env::current_exe().unwrap())
            .with_env("RELAY_INSTALL_PIPE_FIXTURE", mode);
        let mut run = runner
            .install(
                vec![
                    "--ignored".into(),
                    "--exact".into(),
                    HELPER.into(),
                    "--nocapture".into(),
                ],
                None,
            )
            .unwrap();
        let mut seen = Vec::new();
        // All final events are buffered before exit/cancellation; no event may
        // be lost when the supervisor closes a descendant-held stdout reader.
        for _ in 0..3 {
            seen.push(
                tokio::time::timeout(Duration::from_secs(3), run.events.recv())
                    .await
                    .unwrap()
                    .unwrap()["event"]
                    .as_str()
                    .unwrap()
                    .to_owned(),
            );
        }
        assert_eq!(seen, ["authenticated", "connected", "ready"]);
        if mode == "cancel" {
            (run.cancel)();
        }
        let started = std::time::Instant::now();
        assert!(
            tokio::time::timeout(Duration::from_secs(3), run.events.recv())
                .await
                .unwrap_or_else(|_| panic!("event stream stayed open: {mode}"))
                .is_none()
        );
        let result = tokio::time::timeout(Duration::from_secs(3), run.finished)
            .await
            .unwrap()
            .unwrap();
        match mode {
            "error" => assert!(result.is_err()),
            "cancel" => assert!(result.unwrap_err().cancelled),
            _ => assert!(result.is_ok()),
        }
        assert!(started.elapsed() < Duration::from_secs(3), "{mode}");
    }
}
