//! The generated native hook command runs this build's actual CLI. Synthetic
//! non-MCP calls return before opening a socket or signing any identity.
#![cfg(target_os = "macos")]

use agent_relay_node::relay::copilot::{CopilotConfig, CopilotSettings};
use serde_json::{json, Value};
use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn generated_before_command_allows_non_mcp_tools_with_the_compiled_helper() {
    let home = tempfile::tempdir().unwrap();
    let helper = home.path().join("Current App/Agent Relay Helper");
    std::fs::create_dir_all(helper.parent().unwrap()).unwrap();
    std::fs::copy(env!("CARGO_BIN_EXE_agent-relay-desktop-probe"), &helper).unwrap();
    let config = CopilotConfig::from_env(&|_| None, home.path());
    CopilotSettings::new(&config, helper)
        .set_enabled(true)
        .unwrap();
    let hooks: Value =
        serde_json::from_slice(&std::fs::read(config.hooks_file()).unwrap()).unwrap();
    let command = hooks["hooks"]["preToolUse"][0]["command"].as_str().unwrap();
    for tool in ["rename_chat", "tool_search"] {
        let mut child = Command::new("/bin/sh")
            .args(["-c", command])
            // Unrelated tool names return before any profile/socket lookup;
            // no real account variables or home override reach this process.
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let payload = json!({"sessionId":"1069773c-0824-4fad-b7f9-d3dc3c4ed148",
            "cwd":home.path(),"toolName":tool,"toolArgs":{"fixture":true}});
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.to_string().as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{tool}: {output:?}");
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            json!({})
        );
        assert!(output.stderr.is_empty());
    }
}
