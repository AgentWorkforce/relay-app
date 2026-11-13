//! The actual GTK Agents widgets against the isolated production Core host.
use super::*;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};

struct NativeHost {
    child: Child,
    input: ChildStdin,
    output: std::sync::mpsc::Receiver<Result<Value, String>>,
    directory: tempfile::TempDir,
    process_group: Option<i32>,
}
impl NativeHost {
    fn start(executable: &std::ffi::OsStr) -> Self {
        let directory = tempfile::Builder::new()
            .prefix("nv-gtk-")
            .tempdir_in("/tmp")
            .unwrap();
        let mut child = Command::new(executable)
            .arg(directory.path())
            .env_clear()
            .env("HOME", directory.path())
            .env("PATH", "/usr/bin:/bin")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let input = child.stdin.take().unwrap();
        let reader = BufReader::new(child.stdout.take().unwrap());
        let (sender, output) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in reader.lines() {
                let value = line.map_err(|error| error.to_string()).and_then(|line| {
                    serde_json::from_str(&line).map_err(|error| error.to_string())
                });
                if sender.send(value).is_err() {
                    return;
                }
            }
        });
        let mut host = Self {
            child,
            input,
            output,
            directory,
            process_group: None,
        };
        let ready = host.read(Duration::from_secs(10));
        assert_eq!(ready["ready"], true);
        if ready["process_group"].as_u64() == Some(host.child.id() as u64)
            && unsafe { libc::getpgid(host.child.id() as i32) } == host.child.id() as i32
        {
            host.process_group = Some(host.child.id() as i32);
        }
        host
    }
    fn read(&mut self, timeout: Duration) -> Value {
        self.output
            .recv_timeout(timeout)
            .expect("isolated host response timed out or exited")
            .expect("invalid isolated host response")
    }
    fn record(&self, stage: &str, value: Value) {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(self.directory.path().join("stages.jsonl"))
            .unwrap();
        writeln!(file, "{}", json!({"stage":stage,"value":value})).unwrap();
    }
    fn command(&mut self, value: Value) -> Value {
        writeln!(self.input, "{value}").unwrap();
        self.input.flush().unwrap();
        self.read(Duration::from_secs(15))
    }
}
impl Drop for NativeHost {
    fn drop(&mut self) {
        let _ = writeln!(self.input, "{{\"op\":\"stop\"}}");
        let _ = self.input.flush();
        let deadline = Instant::now() + Duration::from_secs(7);
        while Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if let Some(group) = self.process_group {
            unsafe {
                libc::kill(-group, libc::SIGKILL);
            }
        } else if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
        if let Some(output) = std::env::var_os("RELAY_NATIVE_VISIBILITY_ARTIFACTS") {
            let destination =
                std::path::PathBuf::from(output).join(self.directory.path().file_name().unwrap());
            let _ = std::fs::create_dir_all(&destination);
            let _ = std::fs::copy(
                self.directory.path().join("stages.jsonl"),
                destination.join("stages.jsonl"),
            );
        }
    }
}
struct NativeApp(App);
impl Drop for NativeApp {
    fn drop(&mut self) {
        self.0.shutdown();
        self.0.window().destroy();
    }
}
fn pump(until: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        dispatch_one();
        if until() {
            return;
        }
    }
    panic!("production native GTK visibility gate timed out");
}

#[cfg(unix)]
pub fn production_discovery_renders_and_native_switch_registers(runtime: &tokio::runtime::Runtime) {
    let Some(executable) = std::env::var_os("RELAY_NATIVE_VISIBILITY_HOST") else {
        eprintln!("UNAVAILABLE: set RELAY_NATIVE_VISIBILITY_HOST; this skip is not production native acceptance evidence");
        return;
    };
    for (kind, id, title) in [
        (
            "claude-terminal",
            "727cce8d-8074-4ea3-94a0-dea144bf8747",
            "Fresh Claude Orchard",
        ),
        (
            "codex",
            "01a0c221-6806-7210-af25-97e104b15049",
            "Fresh Codex Orchard",
        ),
    ] {
        for mode in ["already-open", "open-after-create", "hidden-reveal"] {
            let mut host = NativeHost::start(&executable);
            let before_open_creation = Instant::now();
            let initial_discovery = if mode == "open-after-create" {
                host.command(json!({"op":"seed", "kind":kind, "id":id, "title":title}));
                Some(host.command(json!({"op":"discover", "kind":kind})))
            } else {
                None
            };
            let mut prefs = Preferences::load(host.directory.path().join("desktop.json"));
            prefs.set_automatically_checks_for_updates(false).unwrap();
            let opened = Instant::now();
            let native = NativeApp(App::new(Options {
                bridge: BridgeOptions {
                    socket: host.directory.path().join("core.sock"),
                    host: None,
                    service: None,
                },
                runtime: runtime.handle().clone(),
                prefs,
                service: Arc::new(FakeService::default()),
                desktop: Rc::new(FakeDesktop {
                    cloud_apps_installed: Cell::new(false),
                    cloud_app_open_fails: Cell::new(false),
                    codex_installed: Cell::new(false),
                    session_plans: RefCell::default(),
                    session_resume_error: RefCell::default(),
                    opened: RefCell::default(),
                    notified: RefCell::default(),
                    asked: RefCell::default(),
                    informed: RefCell::default(),
                    answer: Cell::new(true),
                    choice: Cell::new(Response::Cancel),
                    quit: Cell::new(false),
                }),
                tray: Box::new(RecordingTray::default()),
                install_kind: InstallKind::Source,
                window: gtk::Window::new(),
            }));
            let app = &native.0;
            app.activate();
            pump(|| app.with_state(|state| state.connected()));
            app.go(Page::Sessions);
            assert_eq!(app.page(), Page::Sessions);
            if mode != "open-after-create" {
                pump(|| {
                    app.with_state(|state| {
                        state.session_query.local_sessions.is_some() && !state.refreshing_sessions
                    })
                });
            }
            if mode == "hidden-reveal" {
                app.window().set_visible(false);
            }
            let created = if mode == "open-after-create" {
                before_open_creation
            } else {
                Instant::now()
            };
            if mode != "open-after-create" {
                host.command(json!({"op":"seed", "kind":kind, "id":id, "title":title}));
            }
            let discovered = initial_discovery
                .unwrap_or_else(|| host.command(json!({"op":"discover", "kind":kind})));
            assert!(discovered["ids"].as_array().unwrap().contains(&json!(id)));
            assert!(discovered["sessions"]
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["target"]["id"] == id && row["live"] == true));
            if mode == "hidden-reveal" {
                app.show_window();
            }
            pump(|| {
                app.with_state(|state| state.local_sessions.iter().any(|row| row.target.id == id))
            });
            host.record(
                "gtk_core_visible",
                json!({"id":id,"creation_to_ms":created.elapsed().as_millis()}),
            );
            pump(|| {
                let mut rendered = false;
                walk(app.window().upcast_ref(), &mut |widget| {
                    if let Some(label) = widget.downcast_ref::<gtk::Label>() {
                        if label.text() == title && label.is_mapped() && label.allocated_width() > 0
                        {
                            rendered = true;
                        }
                    }
                });
                rendered
            });
            let target: agent_relay_node::relay::store::Target =
                serde_json::from_value(json!({"kind":kind,"id":id})).unwrap();
            let control_key = target.upload_key().unwrap_or_else(|| target.key());
            let mut controls = Vec::new();
            let mut control_names = Vec::new();
            walk(app.window().upcast_ref(), &mut |widget| {
                if widget.widget_name().starts_with("receive-messages ") {
                    control_names.push(widget.widget_name().to_string());
                }
                if widget.widget_name() == format!("receive-messages {control_key}") {
                    if let Some(control) = widget.downcast_ref::<gtk::Button>() {
                        controls.push(control.clone());
                    }
                }
            });
            // Lookup through the actual rendered widget tree, not local_sessions.
            let control = controls.pop().unwrap_or_else(||panic!("rendered exact-target Receives messages control {control_key}; present={control_names:?}"));
            pump(|| control.is_mapped() && control.allocated_width() > 0);
            assert!(control.is_sensitive());
            assert!(labels(&control).contains(&"Off".to_string()));
            let bounds = control.compute_bounds(app.window()).unwrap();
            assert!(bounds.x() >= 0.0 && bounds.x() + bounds.width() <= app.window().allocated_width() as f32,
                "The real control must be inside the visible window, not clipped in the horizontal scroll area");
            assert!(
                app.with_state(|state| !state.sessions.iter().any(|row| row.session_id == id)),
                "Primary row must be relay-only before history capture"
            );
            let rendered_ms = created.elapsed().as_millis();
            host.record(
                "gtk_native_rendered",
                json!({"id":id,"mode":mode,"creation_to_ms":rendered_ms,"open_to_ms":opened.elapsed().as_millis()}),
            );
            control.emit_clicked();
            pump(|| {
                app.with_state(|state| {
                    state
                        .local_sessions
                        .iter()
                        .any(|row| row.target.id == id && row.registration.is_some())
                })
            });
            let registration = app.with_state(|state| {
                state
                    .local_sessions
                    .iter()
                    .find(|row| row.target.id == id)
                    .unwrap()
                    .registration
                    .clone()
                    .unwrap()
            });
            host.record("gtk_registered", json!({"id":id,"address":registration.address,"creation_to_ms":created.elapsed().as_millis()}));
            let sent = host.command(json!({"op":"send", "name":registration.name, "text":format!("Orchard exact transport {id}")}));
            if kind == "claude-terminal" {
                let approved = host.command(
                    json!({"op":"resolve_fixture_consent","delivery_id":sent["delivery_id"]}),
                );
                assert!(approved["approved"] == true || approved["already_delivered"] == true);
            }
            pump(|| {
                std::fs::read_to_string(host.directory.path().join("stages.jsonl"))
                    .unwrap()
                    .contains(sent["message_id"].as_str().unwrap())
            });
            let receipts =
                std::fs::read_to_string(host.directory.path().join("stages.jsonl")).unwrap();
            let records: Vec<Value> = receipts
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            let received: Vec<_> = records
                .iter()
                .filter(|row| row["stage"] == "transport_accepted")
                .collect();
            assert_eq!(received.len(), 1, "one exact isolated provider handoff");
            let receipt = &received[0]["value"];
            assert_eq!(receipt["id"], id);
            if kind == "claude-terminal" {
                assert_eq!(receipt["auth"]["type"], "auth");
                assert_eq!(receipt["auth"]["token"], "fake-isolated-inbox-token");
                assert_eq!(receipt["message"]["msg_id"], sent["message_id"]);
                assert!(receipt["message"]["message"]["content"]
                    .as_str()
                    .unwrap()
                    .contains(&format!("Orchard exact transport {id}")));
            } else {
                assert_eq!(receipt["message_id"], sent["message_id"]);
                assert!(receipt["text"]
                    .as_str()
                    .unwrap()
                    .contains(&format!("Orchard exact transport {id}")));
            }
            let receipts =
                std::fs::read_to_string(host.directory.path().join("stages.jsonl")).unwrap();
            println!("native GTK {kind}/{mode}: rendered_ms={rendered_ms}, registered_address={}, message={sent}", registration.address);
            if let Some(output) = std::env::var_os("RELAY_NATIVE_VISIBILITY_ARTIFACTS") {
                let output = std::path::PathBuf::from(output);
                std::fs::create_dir_all(&output).unwrap();
                std::fs::write(output.join(format!("gtk-{kind}-{mode}.jsonl")), receipts).unwrap();
            }
        }
    }
}
