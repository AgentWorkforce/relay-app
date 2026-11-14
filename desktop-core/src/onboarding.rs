//! Terminal onboarding for the private, GTK-free desktop core. Authentication
//! stays in the running core: the wizard receives only public device approval
//! instructions and drives the same account/provider/settings RPCs as the app.
use agent_relay_node::relay::{cli, links::built_in_site_url};
use clap::{Parser, ValueEnum};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The Linux systemd user unit that runs the headless core.
const SYSTEMD_UNIT: &str = "agent-relay.service";
/// The macOS headless LaunchAgent label and plist file name, as written by
/// `macos/install.sh`; keep the two in step.
const LAUNCH_AGENT_LABEL: &str = "com.agentrelay.desktop.core.headless";
#[cfg(target_os = "macos")]
const LAUNCH_AGENT_PLIST: &str = "com.agentrelay.desktop.core.headless.plist";

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Sharing {
    Selected,
    New,
    All,
}
impl Sharing {
    fn name(self) -> &'static str {
        match self {
            Self::Selected => "selected",
            Self::New => "new",
            Self::All => "all",
        }
    }
    fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "selected" | "1" => Some(Self::Selected),
            "new" | "2" => Some(Self::New),
            "all" | "3" => Some(Self::All),
            _ => None,
        }
    }
}

#[derive(Debug, Parser)]
#[command(
    name = "agent-relay-probe setup",
    about = "Set up Agent Relay without a desktop window"
)]
struct Options {
    #[arg(long, value_name = "PATH")]
    socket: Option<PathBuf>,
    /// Gone: the site is fixed when the core is built. Kept hidden so the
    /// person who still passes it learns why (`site_url_gone`), as a
    /// result event like any other failure, not clap's generic error.
    #[arg(long, hide = true, value_name = "ORIGIN")]
    site_url: Option<String>,
    #[arg(long, value_name = "UUID")]
    workspace: Option<String>,
    /// Provider IDs, separated by commas; "detected" connects installed providers.
    /// "none" explicitly disconnects local MCP providers. Omit to preserve choices.
    #[arg(long, value_name = "IDS|detected|none")]
    providers: Option<String>,
    /// Omit to preserve the saved upload policy. New installations start selected.
    #[arg(long, value_enum)]
    sharing: Option<Sharing>,
    /// Emit newline-delimited JSON events and a final result; never prompt.
    #[arg(long)]
    json: bool,
    /// Never read answers from stdin.
    #[arg(long)]
    non_interactive: bool,
    /// Wait for device approval and preparation instead of returning pending.
    #[arg(long)]
    wait: bool,
    #[arg(long, default_value_t = 600, value_parser = clap::value_parser!(u64).range(1..=3600))]
    timeout: u64,
    /// Start the existing Linux user service if the core is not running.
    #[arg(long)]
    start_service: bool,
    /// Explicitly allow setup past older upload schedules that could not be inspected.
    #[arg(long)]
    acknowledge_uninspected_schedules: bool,
}

trait Backend {
    fn call(&mut self, method: &'static str, params: Value) -> Result<Value, ()>;
    fn pause(&mut self, duration: Duration);
    fn elapsed(&self) -> Duration;
    fn start_service(&mut self) -> bool;
}
trait Interface {
    fn interactive(&self) -> bool;
    fn event(&mut self, event: &'static str, fields: Value);
    fn prompt(&mut self, question: &str) -> Result<String, ()>;
}
struct LocalCore {
    socket: Option<PathBuf>,
    started: Instant,
}
impl Backend for LocalCore {
    fn call(&mut self, method: &'static str, params: Value) -> Result<Value, ()> {
        // Never copy raw RPC failures into output: auth/provider error messages
        // may originate outside this process. The wizard emits its own reasons.
        cli::call(
            self.socket.as_deref(),
            method,
            params,
            Duration::from_secs(60),
        )
        .map_err(|_| ())
    }
    fn pause(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
    fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }
    fn start_service(&mut self) -> bool {
        #[cfg(target_os = "linux")]
        {
            run_quiet("systemctl", &["--user", "start", SYSTEMD_UNIT])
        }
        #[cfg(target_os = "macos")]
        {
            let target = format!("gui/{}/{LAUNCH_AGENT_LABEL}", nix::unistd::getuid());
            if run_quiet("launchctl", &["kickstart", &target]) {
                return true;
            }
            // Not loaded yet: load the installed plist, then kick it again.
            let Some(home) = std::env::var_os("HOME") else {
                return false;
            };
            let plist = PathBuf::from(home)
                .join("Library/LaunchAgents")
                .join(LAUNCH_AGENT_PLIST);
            let domain = format!("gui/{}", nix::unistd::getuid());
            run_quiet(
                "launchctl",
                &["bootstrap", &domain, &plist.to_string_lossy()],
            ) && run_quiet("launchctl", &["kickstart", &target])
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            false
        }
    }
}
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn run_quiet(program: &str, args: &[&str]) -> bool {
    std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}
struct Terminal {
    json: bool,
    interactive: bool,
}
impl Interface for Terminal {
    fn interactive(&self) -> bool {
        self.interactive
    }
    fn event(&mut self, event: &'static str, mut fields: Value) {
        if self.json {
            fields["event"] = json!(event);
            println!("{fields}");
            return;
        }
        if let Some(message) = fields["message"].as_str() {
            println!("{message}");
        }
        if let Some(url) = fields["verification_url_complete"]
            .as_str()
            .or(fields["verification_url"].as_str())
        {
            println!("Approve this server from any computer: {url}");
        }
        if let Some(code) = fields["user_code"].as_str() {
            println!("Approval code: {code}");
        }
        if let Some(url) = fields["url"].as_str() {
            println!("{url}");
        }
        for action in fields["actions"].as_array().into_iter().flatten() {
            if let Some(message) = action["message"].as_str() {
                println!("{message}");
            }
            if let Some(url) = action["url"].as_str() {
                println!("{url}");
            }
        }
    }
    fn prompt(&mut self, question: &str) -> Result<String, ()> {
        if !self.interactive {
            return Err(());
        }
        eprint!("{question} ");
        io::stderr().flush().map_err(|_| ())?;
        let mut answer = String::new();
        if io::stdin().read_line(&mut answer).map_err(|_| ())? == 0 {
            return Err(());
        }
        Ok(answer.trim().to_string())
    }
}

#[derive(Clone, Copy, Debug)]
struct Failure {
    code: &'static str,
    message: &'static str,
}
impl Failure {
    fn step() -> Self {
        Self { code: "core_request_failed", message: "Agent Relay could not complete this setup step. Check the running core and retry setup." }
    }
    fn choice() -> Self {
        Self {
            code: "invalid_choice",
            message: "That setup choice was not recognized. Use the choices shown by --help.",
        }
    }
}
fn call(core: &mut impl Backend, method: &'static str, params: Value) -> Result<Value, Failure> {
    core.call(method, params).map_err(|_| Failure::step())
}
fn pending(ui: &mut impl Interface, code: &'static str, fields: Value) -> i32 {
    let mut fields = fields;
    fields["status"] = json!("action_required");
    fields["code"] = json!(code);
    ui.event("result", fields);
    2
}
fn auth_ready(report: &Value) -> bool {
    report["state"] == "signed_in"
        && report["needs_sign_in"] != true
        && report["auth_health"] != "sign_in_required"
}
fn approval(report: &Value) -> Value {
    // The core signs in to the site it was built for; a report names it.
    let site = report["site_url"].as_str().unwrap_or(built_in_site_url());
    let origin = |text: &str| {
        url::Url::parse(text)
            .ok()
            .map(|url| url.origin().ascii_serialization())
    };
    let site = origin(site);
    let mut public = json!({"message":"Approve Agent Relay on another device; this server does not need a browser or a forwarded port."});
    for key in ["verification_url", "verification_url_complete"] {
        if let Some(link) = report[key]
            .as_str()
            .filter(|link| origin(link) == site && site.is_some())
        {
            public[key] = json!(link);
        }
    }
    if let Some(code) = report["user_code"].as_str().filter(|code| {
        code.len() <= 32
            && code
                .bytes()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'-')
    }) {
        public["user_code"] = json!(code);
    }
    if let Some(seconds) = report["interval"].as_u64() {
        public["poll_after_seconds"] = json!(seconds.clamp(1, 60));
    }
    public
}
fn sign_in(
    core: &mut impl Backend,
    options: &Options,
    acknowledge: bool,
) -> Result<Value, Failure> {
    call(
        core,
        "account.sign_in",
        json!({"flow":"device", "workspace":options.workspace, "sharing":"selected", "acknowledge_uninspected_schedules":acknowledge}),
    )
}
fn terminal_auth_failure(report: &Value) -> Option<Failure> {
    match report["state"].as_str()? {
        "denied" => Some(Failure { code:"approval_denied", message:"Device approval was denied. Rerun setup when you are ready to approve this server." }),
        "expired" => Some(Failure { code:"approval_expired", message:"Device approval expired. Rerun setup to get a new code." }),
        "error" => Some(Failure { code:"sign_in_failed", message:"Agent Relay sign-in did not finish. Check your account and network, then rerun setup." }),
        _ => None,
    }
}

mod auth;
mod providers;
#[cfg(test)]
use providers::provider_ids;

/// Why `--site-url` is refused: this core signs in to the one site it was
/// built for, and no option changes that.
fn site_url_gone() -> String {
    format!(
        "--site-url is gone: this core is built for {} and signs in there. The site is fixed when \
         the core is built (AGENT_RELAY_SITE_URL at build time); a core for another site is \
         another build.",
        built_in_site_url()
    )
}
fn setup(
    core: &mut impl Backend,
    ui: &mut impl Interface,
    options: &Options,
) -> Result<i32, Failure> {
    ui.event(
        "started",
        json!({"message":"Set up Agent Relay on this computer."}),
    );
    if options.site_url.is_some() {
        ui.event(
            "result",
            json!({"status":"error", "code":"site_url_gone", "message":site_url_gone()}),
        );
        // Status 1 like every other setup failure; 2 is action_required.
        return Ok(1);
    }
    match auth::connect(core, ui, options)? {
        auth::Connection::Ready(account) => providers::configure(core, ui, options, &account),
        auth::Connection::Pending(exit) => Ok(exit),
    }
}

/// Private binary dispatch; help and argument errors never contact the core.
pub fn run(arguments: &[String]) -> i32 {
    let options = match Options::try_parse_from(
        std::iter::once("agent-relay-probe setup".to_string())
            .chain(arguments.iter().cloned()),
    ) {
        Ok(options) => options,
        Err(error) => {
            let exit = error.exit_code();
            let _ = error.print();
            return exit;
        }
    };
    let mut core = LocalCore {
        socket: options.socket.clone(),
        started: Instant::now(),
    };
    let mut terminal = Terminal {
        json: options.json,
        interactive: !options.non_interactive
            && !options.json
            && io::stdin().is_terminal()
            && io::stdout().is_terminal(),
    };
    match setup(&mut core, &mut terminal, &options) {
        Ok(exit) => exit,
        Err(failure) => {
            terminal.event(
                "result",
                json!({"status":"error", "code":failure.code, "message":failure.message}),
            );
            1
        }
    }
}

#[cfg(test)]
mod tests;
