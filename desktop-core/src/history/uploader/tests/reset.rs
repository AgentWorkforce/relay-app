use super::*;

/// Records the deadline each command ran with and confirms resets.
struct DeadlineProbe {
    deadlines: Mutex<Vec<(String, Option<Duration>)>>,
}

impl ProbeRunner for DeadlineProbe {
    fn status(
        &self,
        _: Option<std::path::PathBuf>,
        _: crate::history::uploader::HistoryReader,
    ) -> BoxFuture<'_, Result<Value, UploaderError>> {
        self.run(vec!["status".into()], None)
    }
    fn run(
        &self,
        args: Vec<String>,
        timeout: Option<Duration>,
    ) -> BoxFuture<'_, Result<Value, UploaderError>> {
        Box::pin(async move {
            self.deadlines
                .lock()
                .unwrap()
                .push((args[..2.min(args.len())].join(" "), timeout));
            Ok(json!({ "reset": true }))
        })
    }

    fn install(
        &self,
        _args: Vec<String>,
        _bearer: Option<Secret>,
    ) -> Result<InstallRun, UploaderError> {
        Err(UploaderError::new("unused"))
    }

    fn cancel_all(&self) -> BoxFuture<'_, Result<(), UploaderError>> {
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn a_reset_runs_with_a_deadline_because_it_holds_every_reader() {
    let probe = Arc::new(DeadlineProbe {
        deadlines: Mutex::new(Vec::new()),
    });
    let uploader = Uploader::new(probe.clone());
    uploader.attach(Install::from_json(
        &json!({ "site_url": "https://example.test" }),
    ));
    uploader
        .sessions_reset("DELETE ALL SESSIONS")
        .await
        .unwrap();
    assert_eq!(
        *probe.deadlines.lock().unwrap(),
        vec![("sessions reset".to_string(), Some(timeouts::RESET))]
    );
}

#[test]
fn a_reset_past_its_deadline_says_sessions_may_be_gone() {
    // The other `sessions` commands share the first argument; only the
    // reset may have deleted anything by the time it is stopped.
    let target = Target {
        site_url: "https://example.test".into(),
        account_id: None,
        workspace_id: None,
    };
    let reset = command::sessions_reset(&target, "DELETE ALL SESSIONS");
    assert_eq!(
        timeouts::message(&reset),
        "Deleting your sessions took too long. Some may already be gone; run the reset again to finish it."
    );
    let delete = command::sessions_delete(&target, &["claude:1".into()]);
    assert_eq!(timeouts::message(&delete), timeout_message("sessions"));
    assert_eq!(
        timeouts::message(&["compact".into()]),
        timeout_message("compact")
    );
}
