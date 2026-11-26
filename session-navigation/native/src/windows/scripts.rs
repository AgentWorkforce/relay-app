//! Every script the Windows adapter runs. Each is a constant (assembled from
//! constant fragments with `concat!`): data reaches a PowerShell script only
//! as JSON on stdin, and reaches the WSL `sh` scripts only as positional
//! words. Nothing is ever spliced into source, except the environment
//! wrapper's payload, which is Base64 (`[A-Za-z0-9+/=]` only).
//!
//! All of them are UNVERIFIED: none has run on Windows. Each is checked by
//! `tests/windows_native_acceptance.rs` on a real machine (PowerShell's own
//! parser, then a read-only observe).

/// Strict errors, no progress records, UTF-8 output without a BOM.
macro_rules! prelude {
    () => {
        "$ErrorActionPreference = 'Stop'\n\
$ProgressPreference = 'SilentlyContinue'\n\
[Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false\n"
    };
}

/// The boot id: the kernel's boot counter (`PrefetchParameters\BootId`,
/// incremented on every boot; stable across clock corrections, unlike
/// `LastBootUpTime`, which moves when the clock is corrected), else
/// `LastBootUpTime` in Unix seconds. `$bootTime` is `LastBootUpTime`.
/// UNVERIFIED: the registry value's presence on every edition.
macro_rules! boot {
    () => {
        "$bootId = $null\n\
$bootTime = $null\n\
try { $bootTime = (Get-CimInstance -ClassName Win32_OperatingSystem).LastBootUpTime } catch { }\n\
try { $bootId = [long](Get-ItemProperty -LiteralPath 'HKLM:\\SYSTEM\\CurrentControlSet\\Control\\Session Manager\\Memory Management\\PrefetchParameters' -Name BootId).BootId } catch { }\n"
    };
}

/// `$processes`: the whole `Win32_Process` table, or `$null`.
macro_rules! processes {
    () => {
        "$processes = $null\n\
try { $processes = @(Get-CimInstance -ClassName Win32_Process | Select-Object ProcessId, ParentProcessId, CreationDate, Name, ExecutablePath, CommandLine, SessionId) } catch { }\n"
    };
}

/// The C# helper: visible unowned top-level windows with their owner's pid
/// and creation time (Unix seconds, -1 when the process cannot be opened),
/// the foreground window, and the act of raising one. C# 5 only (the
/// compiler `Add-Type` uses on .NET Framework 4.x).
macro_rules! native {
    () => {
        "$sn = @'\n\
using System;\n\
using System.Collections.Generic;\n\
using System.Runtime.InteropServices;\n\
public static class SessionNavigationNative {\n\
  public delegate bool EnumProc(IntPtr hwnd, IntPtr lparam);\n\
  [DllImport(\"user32.dll\")] public static extern bool EnumWindows(EnumProc f, IntPtr l);\n\
  [DllImport(\"user32.dll\")] public static extern bool IsWindowVisible(IntPtr h);\n\
  [DllImport(\"user32.dll\")] public static extern bool IsWindow(IntPtr h);\n\
  [DllImport(\"user32.dll\")] public static extern bool IsIconic(IntPtr h);\n\
  [DllImport(\"user32.dll\")] public static extern IntPtr GetWindow(IntPtr h, uint cmd);\n\
  [DllImport(\"user32.dll\")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);\n\
  [DllImport(\"user32.dll\")] public static extern IntPtr GetForegroundWindow();\n\
  [DllImport(\"user32.dll\")] public static extern bool SetForegroundWindow(IntPtr h);\n\
  [DllImport(\"user32.dll\")] public static extern bool ShowWindow(IntPtr h, int cmd);\n\
  [DllImport(\"kernel32.dll\")] public static extern IntPtr OpenProcess(uint access, bool inherit, uint pid);\n\
  [DllImport(\"kernel32.dll\")] public static extern bool CloseHandle(IntPtr h);\n\
  [DllImport(\"kernel32.dll\")] public static extern bool GetProcessTimes(IntPtr h, out long c, out long e, out long k, out long u);\n\
  public static long Created(uint pid) {\n\
    IntPtr h = OpenProcess(0x1000, false, pid);\n\
    if (h == IntPtr.Zero) return -1;\n\
    try { long c, e, k, u; if (!GetProcessTimes(h, out c, out e, out k, out u)) return -1; return c / 10000000L - 11644473600L; }\n\
    finally { CloseHandle(h); }\n\
  }\n\
  public static long Owner(IntPtr h) { uint pid; GetWindowThreadProcessId(h, out pid); return pid; }\n\
  public static List<long[]> Windows() {\n\
    List<long[]> list = new List<long[]>();\n\
    EnumWindows(delegate (IntPtr h, IntPtr l) {\n\
      if (IsWindowVisible(h) && GetWindow(h, 4) == IntPtr.Zero) { long pid = Owner(h); list.Add(new long[] { h.ToInt64(), pid, Created((uint)pid) }); }\n\
      return true;\n\
    }, IntPtr.Zero);\n\
    return list;\n\
  }\n\
}\n\
'@\n\
if (-not ('SessionNavigationNative' -as [type])) { Add-Type -TypeDefinition $sn }\n"
    };
}

/// The observe report: boot, the process table, the windows, the
/// foreground window, Windows Terminal's package and the default terminal
/// application (`HKCU\Console\%%Startup` `DelegationConsole` /
/// `DelegationTerminal`; a missing key or value is `''`, "let Windows
/// decide"). Each section fails on its own (`null`), never the whole report.
pub const OBSERVE: &str = concat!(
    prelude!(),
    boot!(),
    processes!(),
    "$windows = $null\n\
$foreground = $null\n\
try {\n",
    native!(),
    "  $windows = @([SessionNavigationNative]::Windows() | ForEach-Object { [ordered]@{ Handle = $_[0]; ProcessId = $_[1]; Created = $_[2] } })\n\
  $fg = [SessionNavigationNative]::GetForegroundWindow()\n\
  $fgPid = [SessionNavigationNative]::Owner($fg)\n\
  $foreground = [ordered]@{ Handle = $fg.ToInt64(); ProcessId = $fgPid; Created = [SessionNavigationNative]::Created([uint32]$fgPid) }\n\
} catch { }\n\
$wt = $null\n\
try {\n\
  $pkg = Get-AppxPackage -Name Microsoft.WindowsTerminal | Select-Object -First 1\n\
  if ($pkg) { $wt = [ordered]@{ Installed = $true; Version = [string]$pkg.Version } } else { $wt = [ordered]@{ Installed = $false; Version = $null } }\n\
} catch { }\n\
$delegation = $null\n\
try {\n\
  $k = 'HKCU:\\Console\\%%Startup'\n\
  if (Test-Path -LiteralPath $k) { $p = Get-ItemProperty -LiteralPath $k; $delegation = [ordered]@{ Console = [string]$p.DelegationConsole; Terminal = [string]$p.DelegationTerminal } } else { $delegation = [ordered]@{ Console = ''; Terminal = '' } }\n\
} catch { }\n\
$out = [ordered]@{ bootId = $bootId; bootTime = $bootTime; processes = $processes; windows = $windows; foreground = $foreground; terminal = $wt; delegation = $delegation }\n\
ConvertTo-Json -InputObject $out -Depth 5 -Compress\n"
);

/// Boot and the process table only (the executor's identity re-check).
pub const IDENTITY: &str = concat!(
    prelude!(),
    boot!(),
    processes!(),
    "$out = [ordered]@{ bootId = $bootId; bootTime = $bootTime; processes = $processes }\n\
ConvertTo-Json -InputObject $out -Depth 5 -Compress\n"
);

/// stdin: `{hwnd, owner, ownerStart, process, processStart}`. Re-checks the
/// window's existence and owner (pid and creation time) and the agent's
/// identity at the moment of acting, restores a minimised window, raises it
/// and reads the foreground back. Prints `{"status": "gone" | "refused" |
/// "unconfirmed" | "focused"}`.
pub const FOCUS: &str = concat!(
    prelude!(),
    "$in = [Console]::In.ReadToEnd() | ConvertFrom-Json\n",
    native!(),
    "$h = [IntPtr][long]$in.hwnd\n\
function Done([string]$s) { ConvertTo-Json -InputObject ([ordered]@{ status = $s }) -Compress; exit 0 }\n\
if (-not [SessionNavigationNative]::IsWindow($h)) { Done 'gone' }\n\
$owner = [SessionNavigationNative]::Owner($h)\n\
if ($owner -ne [long]$in.owner) { Done 'gone' }\n\
if ([SessionNavigationNative]::Created([uint32]$owner) -ne [long]$in.ownerStart) { Done 'gone' }\n\
if ([SessionNavigationNative]::Created([uint32]$in.process) -ne [long]$in.processStart) { Done 'gone' }\n\
if ([SessionNavigationNative]::IsIconic($h)) { [void][SessionNavigationNative]::ShowWindow($h, 9) }\n\
if (-not [SessionNavigationNative]::SetForegroundWindow($h)) { Done 'refused' }\n\
Start-Sleep -Milliseconds 150\n\
if ([SessionNavigationNative]::GetForegroundWindow() -ne $h) { Done 'unconfirmed' }\n\
Done 'focused'\n"
);

/// stdin: `{file, line, cwd, env}`. Rebuilds the person's environment
/// (the adapter's runs start with almost none: the volatile, machine and
/// user variables, `PATH` = machine `;` user), applies `env` on top, then
/// `Start-Process`: a console program gets a new console window, `wt.exe`
/// hands the tab to Windows Terminal. `line` is the complete, already
/// quoted argument string. Prints `{"status": "dispatched"}`.
pub const LAUNCH: &str = concat!(
    prelude!(),
    "$in = [Console]::In.ReadToEnd() | ConvertFrom-Json\n\
try { $v = Get-ItemProperty -LiteralPath 'HKCU:\\Volatile Environment'; foreach ($p in $v.PSObject.Properties) { if ($p.Name -notlike 'PS*') { [Environment]::SetEnvironmentVariable($p.Name, [string]$p.Value, 'Process') } } } catch { }\n\
$paths = @()\n\
foreach ($scope in 'Machine', 'User') {\n\
  $vars = [Environment]::GetEnvironmentVariables($scope)\n\
  foreach ($k in $vars.Keys) {\n\
    $val = [Environment]::ExpandEnvironmentVariables([string]$vars[$k])\n\
    if ($k -eq 'Path') { $paths += $val } else { [Environment]::SetEnvironmentVariable($k, $val, 'Process') }\n\
  }\n\
}\n\
if ($paths.Count -gt 0) { [Environment]::SetEnvironmentVariable('Path', ($paths -join ';'), 'Process') }\n\
if ($in.env) { foreach ($p in $in.env.PSObject.Properties) { [Environment]::SetEnvironmentVariable($p.Name, [string]$p.Value, 'Process') } }\n\
$start = @{ FilePath = [string]$in.file; WorkingDirectory = [string]$in.cwd }\n\
if ([string]$in.line -ne '') { $start.ArgumentList = [string]$in.line }\n\
Start-Process @start\n\
ConvertTo-Json -InputObject ([ordered]@{ status = 'dispatched' }) -Compress\n"
);

/// The environment wrapper, run as a Windows Terminal tab's process when a
/// launch carries provider-home variables (a tab does not inherit the
/// launcher's environment). Preceded by `$payload='<Base64 JSON>';`; starts
/// `file line` in `cwd` with `env` in the same console and waits for it.
pub const ENV_WRAPPER: &str = "$ErrorActionPreference = 'Stop'\n\
$d = [Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($payload)) | ConvertFrom-Json\n\
if ($d.env) { foreach ($p in $d.env.PSObject.Properties) { [Environment]::SetEnvironmentVariable($p.Name, [string]$p.Value, 'Process') } }\n\
$si = New-Object System.Diagnostics.ProcessStartInfo\n\
$si.FileName = [string]$d.file\n\
$si.Arguments = [string]$d.line\n\
$si.WorkingDirectory = [string]$d.cwd\n\
$si.UseShellExecute = $false\n\
$child = [System.Diagnostics.Process]::Start($si)\n\
$child.WaitForExit()\n\
exit $child.ExitCode\n";

/// `sh -s` inside a WSL distribution: boot id, `btime`, `CLK_TCK`, the uid
/// and one `proc` line per process: `pid ruid euid stat exe cwd cmdline`,
/// the last four as hex (`-` when unreadable). Read from `/proc` with POSIX
/// `sh`, `od`, `tr` and `readlink` only.
pub const WSL_TABLE: &str = r#"LC_ALL=C; export LC_ALL
hex() { od -An -v -tx1 | tr -d ' \n'; }
printf 'boot %s\n' "$(cat /proc/sys/kernel/random/boot_id)"
while read -r k v r; do if [ "$k" = btime ]; then printf 'btime %s\n' "$v"; fi; done < /proc/stat
printf 'hz %s\n' "$(getconf CLK_TCK)"
printf 'uid %s\n' "$(id -u)"
for d in /proc/[0-9]*; do
  p=${d#/proc/}
  s=$(hex 2>/dev/null < "$d/stat")
  [ -n "$s" ] || continue
  u=-
  while read -r k a b r; do if [ "$k" = Uid: ]; then u="$a $b"; break; fi; done 2>/dev/null < "$d/status"
  [ "$u" = - ] && continue
  x=$(readlink "$d/exe" 2>/dev/null); e=$(printf %s "$x" | hex)
  x=$(readlink "$d/cwd" 2>/dev/null); c=$(printf %s "$x" | hex)
  a=$(hex 2>/dev/null < "$d/cmdline")
  printf 'proc %s %s %s %s %s %s\n' "$p" "$u" "$s" "${e:--}" "${c:--}" "${a:--}"
done
"#;

/// `sh -s -- <dir>… -- <tool>…` inside a WSL distribution: for each tool,
/// `tool <name> <path hex> <version line hex>` or `tool <name> -`. Grok is
/// never run: its canonical file name carries the version.
pub const WSL_TOOLS: &str = r#"LC_ALL=C; export LC_ALL
hex() { od -An -v -tx1 | tr -d ' \n'; }
dirs=
while [ $# -gt 0 ] && [ "$1" != -- ]; do dirs="$dirs $1"; shift; done
[ $# -gt 0 ] && shift
for t in "$@"; do
  f=-
  for d in $dirs; do if [ -f "$d/$t" ] && [ -x "$d/$t" ]; then f="$d/$t"; break; fi; done
  if [ "$f" = - ]; then printf 'tool %s -\n' "$t"; continue; fi
  case "$t" in
    grok) v=$(basename "$(readlink -f "$f")") ;;
    tmux) v=$("$f" -V 2>/dev/null < /dev/null | head -n 1) ;;
    *) v=$("$f" --version 2>/dev/null < /dev/null | head -n 1) ;;
  esac
  printf 'tool %s %s %s\n' "$t" "$(printf %s "$f" | hex)" "$(printf %s "$v" | hex)"
done
"#;
