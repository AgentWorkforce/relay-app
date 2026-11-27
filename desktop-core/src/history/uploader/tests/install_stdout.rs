use super::*;
use std::io::Write;
const HELPER: &str = "history::uploader::install_stdout_tests::helper_process";
const ERROR: &str = "synthetic install failed: could not prepare local sessions";
#[test]
#[ignore = "synthetic subprocess invoked by the install lifecycle regression"]
fn helper_process() {
    let mode = std::env::var("RELAY_INSTALL_PIPE_FIXTURE").unwrap();
    if mode == "hold" {
        std::thread::sleep(Duration::from_secs(6));
        return;
    }
    if mode == "write" {
        let until = std::time::Instant::now() + Duration::from_secs(6);
        let mut output = std::io::stdout().lock();
        while std::time::Instant::now() < until {
            if writeln!(output, "{}", json!({"event":"descendant"})).is_err()
                || output.flush().is_err()
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        return;
    }
    for event in ["authenticated", "connected", "ready"] {
        println!("{}", json!({"event": event}));
    }
    std::io::stdout().flush().unwrap();
    if mode != "eof" {
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", HELPER, "--nocapture"])
            .env(
                "RELAY_INSTALL_PIPE_FIXTURE",
                if mode == "sustained" { "write" } else { "hold" },
            )
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
    }
    match mode.as_str() {
        "error" => {
            eprintln!("{ERROR}");
            std::io::stderr().flush().unwrap();
            std::process::exit(23);
        }
        "cancel" => std::thread::sleep(Duration::from_secs(10)),
        _ => {}
    }
}
#[tokio::test]
async fn install_closes_inherited_stdout_after_success_error_and_cancellation() {
    for mode in ["eof", "success", "error", "cancel", "sustained"] {
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
        // Final helper events precede descendant output and must arrive in
        // order before stream closure, whether the pipe reaches EOF or not.
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
        let extra = tokio::time::timeout(Duration::from_secs(3), async {
            let mut extra = 0;
            while let Some(event) = run.events.recv().await {
                assert_eq!(mode, "sustained");
                assert_eq!(event["event"], "descendant");
                extra += 1;
            }
            extra
        })
        .await
        .unwrap_or_else(|_| panic!("event stream stayed open: {mode}"));
        if mode == "sustained" {
            assert!(extra > 0, "fixture must keep writing during the drain");
        }
        let result = tokio::time::timeout(Duration::from_secs(3), run.finished)
            .await
            .unwrap()
            .unwrap();
        match mode {
            "error" => {
                let error = result.unwrap_err();
                assert!(
                    !error.cancelled,
                    "a helper failure is not user cancellation"
                );
                assert_eq!(error.message, ERROR);
            }
            "cancel" => assert!(result.unwrap_err().cancelled),
            _ => assert!(result.is_ok()),
        }
        assert!(started.elapsed() < Duration::from_secs(3), "{mode}");
    }
}
