//! PowerShell sees a fixed program and a Base64-only data envelope. Every value
//! inside the envelope is independently encoded, including each quoted argv item.
use super::*;
use base64::{engine::general_purpose::STANDARD, Engine};

const PROGRAM: &str = r#"
$ErrorActionPreference='Stop';
function Decode([string] $value) { [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($value)) }
$data=Decode $payload | ConvertFrom-Json;
$resume=New-Object System.Diagnostics.ProcessStartInfo;
$resume.UseShellExecute=$false;
$resume.FileName=Decode $data.executable;
$resume.Arguments=($data.arguments | ForEach-Object { Decode $_ }) -join ' ';
$resume.WorkingDirectory=Decode $data.cwd;
foreach ($key in $data.clear) { $resume.EnvironmentVariables.Remove((Decode $key)) }
foreach ($entry in $data.environment) { $resume.EnvironmentVariables[(Decode $entry.key)]=Decode $entry.value }
$child=[System.Diagnostics.Process]::Start($resume);
$child.WaitForExit();exit $child.ExitCode
"#;

pub(in super::super) fn windows_environment_program(command: &SessionResumeCommand) -> Vec<String> {
    let payload = serde_json::json!({
        "executable": STANDARD.encode(&command.executable),
        "arguments": command.args.iter().map(|arg| STANDARD.encode(windows_argument(arg))).collect::<Vec<_>>(),
        "cwd": STANDARD.encode(&command.cwd),
        "clear": PROVIDER_ENVIRONMENT_KEYS.iter().map(|key| STANDARD.encode(key)).collect::<Vec<_>>(),
        "environment": command.environment.as_ref().into_iter().flatten().map(|(key, value)| {
            serde_json::json!({"key": STANDARD.encode(key), "value": STANDARD.encode(value)})
        }).collect::<Vec<_>>()
    });
    // Only the Base64 alphabet crosses into source; no path, key, or argv can
    // supply a PowerShell quote (including U+2019) or a statement delimiter.
    let script = format!(
        "$payload='{}';{PROGRAM}",
        STANDARD.encode(payload.to_string())
    );
    let bytes: Vec<_> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    vec![
        "powershell.exe".into(),
        "-NoLogo".into(),
        "-NoProfile".into(),
        "-NonInteractive".into(),
        "-EncodedCommand".into(),
        STANDARD.encode(bytes),
    ]
}

pub(in super::super) fn windows_argument(value: &str) -> String {
    let mut result = String::from("\"");
    let mut slashes = 0;
    for character in value.chars() {
        if character == '\\' {
            slashes += 1;
            continue;
        }
        let count = if character == '"' {
            slashes * 2 + 1
        } else {
            slashes
        };
        result.extend(std::iter::repeat_n('\\', count));
        result.push(character);
        slashes = 0;
    }
    result.extend(std::iter::repeat_n('\\', slashes * 2));
    result.push('"');
    result
}
