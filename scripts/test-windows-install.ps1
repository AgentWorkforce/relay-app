<#
.SYNOPSIS
test-windows-install.ps1 -- exercise windows/install.ps1 end to end on Windows
with releases served from a local folder in place of the published ones.

.DESCRIPTION
The Windows counterpart of scripts/test-linux-install.sh. A fake core (a small
C# program compiled here with the .NET Framework's csc.exe) reports a version,
runs as the logon task and answers status.get only while it runs, so the
script's promises are checked across the switch-and-restart boundary: a first
install, an idempotent rerun, an update, -Rollback, a new version that never
answers (rolled back), both onto an earlier release's core that takes only the
`relay` form, a bad checksum and a mislabeled core (refused, nothing
changed), a dry run, the app's own task (left to the app), and a site
profile's copy on the cloud release feed (test-windows-install-cloud.ps1).
With -ProbeZip,
the real core from the build is installed the same way and must answer
status.get from the task, through a socket and data directory of its own.

Everything goes under a prefix, task name, socket and data directory of their
own, removed at the end, so an Agent Relay installed for the same account is
left alone; the real core still keeps its command-line copy and session-socket
pointer under the profile, so run this on CI or a throwaway account.

  scripts/test-windows-install.ps1
  scripts/test-windows-install.ps1 -ProbeZip out\AgentRelay-0.0.0-Windows-x64-probe.zip -Version 0.0.0
#>
[CmdletBinding()]
param(
    # The build's AgentRelay-<version>-Windows-<arch>-probe.zip; without it
    # only the fake core is exercised.
    [string]$ProbeZip = '',
    # The version that zip's core reports.
    [string]$Version = '0.0.0'
)

Set-StrictMode -Version 2
$ErrorActionPreference = 'Stop'

$repo = Split-Path -Parent $PSScriptRoot
$install = Join-Path $repo 'windows\install.ps1'
$arch = if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'arm64' } else { 'x64' }
$root = Join-Path ([IO.Path]::GetTempPath()) ("agent-relay-install-test-" + [IO.Path]::GetRandomFileName())
$releases = Join-Path $root 'releases'
$prefix = Join-Path $root 'core'
$state = Join-Path $root 'state'
$task = "Agent Relay install test ($env:USERNAME)"
$probe = Join-Path $prefix 'current\agent-relay-probe.exe'
$socket = Join-Path $root 'core.sock'
$data = Join-Path $root 'data'
$csc = Join-Path $env:WINDIR 'Microsoft.NET\Framework64\v4.0.30319\csc.exe'
if (-not (Test-Path -LiteralPath $csc)) { $csc = Join-Path $env:WINDIR 'Microsoft.NET\Framework\v4.0.30319\csc.exe' }

function Fail([string]$Text) { Write-Host "FAIL: $Text" -ForegroundColor Red; exit 1 }
function Pass([string]$Text) { Write-Host "ok - $Text" }

# A fake core: reports its version; `serve` records it and holds a
# lock for as long as it runs (a broken one exits at once); `call
# status.get` answers with the recorded version only while the lock is held.
# An earlier release's core knows its commands only after the `relay` word, so
# it refuses `serve --help` and `call status.get` without it.
$fakeSource = @'
using System;
using System.IO;
using System.Threading;
class FakeCore {
    const string Version = "@VERSION@";
    const string Root = @"@ROOT@";
    const bool Broken = @BROKEN@;
    const bool Earlier = @EARLIER@;
    static int Main(string[] args) {
        if (args.Length > 0 && args[0] == "--version") { Console.WriteLine("agent-relay-probe " + Version); return 0; }
        if (Earlier && (args.Length == 0 || args[0] != "relay")) { Console.Error.WriteLine("error: unrecognized subcommand"); return 2; }
        // Like the real core, it drops the `relay` word earlier releases needed.
        if (args.Length > 1 && args[0] == "relay") { string[] rest = new string[args.Length - 1]; Array.Copy(args, 1, rest, 0, rest.Length); args = rest; }
        if (args.Length > 1 && args[0] == "serve" && args[1] == "--help") { return 0; }
        if (args.Length > 0 && args[0] == "serve") {
            if (Broken) { Console.Error.WriteLine("broken"); return 1; }
            Directory.CreateDirectory(Root);
            File.WriteAllText(Path.Combine(Root, "running"), Version);
            using (new FileStream(Path.Combine(Root, "lock"), FileMode.Create, FileAccess.ReadWrite, FileShare.None)) {
                Thread.Sleep(Timeout.Infinite);
            }
            return 0;
        }
        if (args.Length > 1 && args[0] == "call" && args[1] == "status.get") {
            try { using (new FileStream(Path.Combine(Root, "lock"), FileMode.Open, FileAccess.Read, FileShare.None)) { } return 1; }
            catch (FileNotFoundException) { return 1; }
            catch (DirectoryNotFoundException) { return 1; }
            catch (IOException) { }
            string running = File.ReadAllText(Path.Combine(Root, "running")).Trim();
            Console.WriteLine("{\"result\":{\"daemon\":{\"version\":\"" + running + "\",\"pid\":1},\"account\":{\"workspace_id\":\"ws_fixture\",\"person\":{\"id\":\"user_fixture\"}}}}");
            return 0;
        }
        // The cloud release feed: headers for this account, and a feed verify
        // that answers from a file (or accepts) and logs what it was asked.
        if (args.Length > 0 && (args[0] == "feed-headers" || (args.Length > 1 && args[0] == "feed" && args[1] == "verify"))) {
            Directory.CreateDirectory(Root);
            File.AppendAllText(Path.Combine(Root, "feed.log"), string.Join(" ", args) + "\n");
            if (args[0] == "feed-headers") {
                string profile = "";
                for (int i = 0; i + 1 < args.Length; i++) { if (args[i] == "--profile") { profile = args[i + 1]; } }
                Console.WriteLine("{\"Authorization\":\"Bearer core-fixture-credential\",\"X-Agent-Relay-Profile\":\"" + profile + "\",\"X-Agent-Relay-Desktop\":\"1\"}");
                return 0;
            }
            string answerPath = Path.Combine(Root, "verify-answer");
            string answer = File.Exists(answerPath) ? File.ReadAllText(answerPath).Trim() : "{\"ok\":true}";
            Console.WriteLine(answer);
            return answer.Contains("\"ok\":true") ? 0 : 1;
        }
        if (args.Length > 1 && args[0] == "feed" && args[1] == "--help") { return 0; }
        return 0;
    }
}
'@

function FakeZip([string]$FakeVersion, [switch]$Broken, [switch]$Earlier) {
    $build = Join-Path $root "build-$FakeVersion"
    if ($Earlier) { $build += '-earlier' }
    $helpers = Join-Path $build 'agent_relay\helpers'
    New-Item -ItemType Directory -Path $helpers -Force | Out-Null
    $source = Join-Path $build 'fake.cs'
    # Variable names are case-insensitive: not $broken, which is the switch.
    $brokenText = if ($Broken) { 'true' } else { 'false' }
    $earlierText = if ($Earlier) { 'true' } else { 'false' }
    [IO.File]::WriteAllText($source, $fakeSource.Replace('@VERSION@', $FakeVersion).Replace('@ROOT@', $state).Replace('@BROKEN@', $brokenText).Replace('@EARLIER@', $earlierText))
    $exe = Join-Path $helpers 'agent-relay-probe.exe'
    & $csc /nologo /target:exe "/out:$exe" $source | Out-Null
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $exe)) { Fail "csc could not build the fake core $FakeVersion" }
    # The published archive carries the license beside the helpers folder.
    [IO.File]::WriteAllText((Join-Path $build 'agent_relay\LICENSE.txt'), "fake license`n")
    $zip = Join-Path $build 'probe.zip'
    Compress-Archive -Path (Join-Path $build 'agent_relay') -DestinationPath $zip
    return $zip
}

function Release([string]$ReleaseVersion, [string]$ZipPath, [switch]$BadChecksum) {
    $dir = Join-Path $releases "v$ReleaseVersion"
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
    $asset = "AgentRelay-$ReleaseVersion-Windows-$arch-probe.zip"
    Copy-Item -LiteralPath $ZipPath -Destination (Join-Path $dir $asset) -Force
    $hash = (Get-FileHash -LiteralPath (Join-Path $dir $asset) -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($BadChecksum) { $hash = ('0' * 64) }
    [IO.File]::WriteAllText((Join-Path $dir "$asset.sha256"), "$hash  $asset`n")
    [IO.File]::WriteAllText((Join-Path $releases 'latest'), "{`"tag_name`": `"v$ReleaseVersion`"}")
}

function RunInstall([string[]]$Arguments = @(), [int]$HealthTimeout = 30) {
    $ErrorActionPreference = 'Continue'
    $env:AGENT_RELAY_RELEASES_API = $releases
    $env:AGENT_RELAY_DOWNLOAD_BASE = $releases
    $env:AGENT_RELAY_PREFIX = $prefix
    $env:AGENT_RELAY_TASK = $task
    $env:AGENT_RELAY_SERVE_ARGS = "--socket `"$socket`" --app-data-dir `"$data`""
    $env:AGENT_RELAY_CORE_SOCKET = $socket
    $env:AGENT_RELAY_HEALTH_TIMEOUT = [string]$HealthTimeout
    $output = & powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $install @Arguments 2>&1 | ForEach-Object { $_.ToString() }
    return @{ code = $LASTEXITCODE; text = ($output -join "`n") }
}

function TaskXml {
    $ErrorActionPreference = 'Continue'
    $xml = & schtasks.exe /Query /TN $task /XML 2>$null
    if ($LASTEXITCODE -ne 0) { return '' }
    return ($xml -join "`n")
}

# What the running core reports, or nothing, asked in the `relay call` form
# every release's core takes, an earlier release's included.
function RunningVersion {
    $ErrorActionPreference = 'Continue'
    $env:AGENT_RELAY_CORE_SOCKET = $socket
    try {
        $answer = & $probe relay call status.get 2>$null | ConvertFrom-Json
        return [string]$answer.result.daemon.version
    } catch { return '' }
}

function Installed { return @(Get-ChildItem -LiteralPath (Join-Path $prefix 'versions') -Directory | ForEach-Object { $_.Name } | Sort-Object) }

function StopTask {
    $ErrorActionPreference = 'Continue'
    & schtasks.exe /End /TN $task 2>$null | Out-Null
    & schtasks.exe /Delete /TN $task /F 2>$null | Out-Null
    Get-Process -Name agent-relay-probe -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -and $_.Path.StartsWith($root, [StringComparison]::OrdinalIgnoreCase) } |
        Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 1
}

function Reset {
    StopTask
    foreach ($folder in $prefix, $state, $data, $releases) {
        if (Test-Path -LiteralPath $folder) { Remove-Item -LiteralPath $folder -Recurse -Force }
    }
    New-Item -ItemType Directory -Path $releases -Force | Out-Null
}

function Cleanup {
    StopTask
    Remove-Item -LiteralPath $root -Recurse -Force -ErrorAction SilentlyContinue
}

$foreign = @(Get-Process -Name agent-relay-probe, agent-relay -ErrorAction SilentlyContinue | Where-Object { -not ($_.Path -and $_.Path.StartsWith($root, [StringComparison]::OrdinalIgnoreCase)) })
if ($foreign.Count -gt 0) { Fail 'Agent Relay is running for this account; run this on CI or a throwaway account' }
if (-not (Test-Path -LiteralPath $csc)) { Fail "csc.exe (the .NET Framework) is needed to build the fake core; looked at $csc" }

New-Item -ItemType Directory -Path $root, $releases -Force | Out-Null
try {
    # --- the fake core: every promise across the switch-and-restart boundary ---
    $zip100 = FakeZip '1.0.0'
    $zip101 = FakeZip '1.0.1'
    $zip102 = FakeZip '1.0.2' -Broken
    $null = FakeZip '1.0.0' -Earlier

    Release '1.0.0' $zip100

    # A dry run on a clean account leaves nothing behind.
    $result = RunInstall @('-DryRun')
    if ($result.code -ne 0 -or $result.text -notmatch 'would install') { Fail "dry run before any install ($($result.code)):`n$($result.text)" }
    if (Test-Path -LiteralPath $prefix) { Fail 'a dry run created the install folder' }
    if (TaskXml) { Fail 'a dry run registered the task' }
    Pass 'a dry run on a clean account writes nothing'

    $result = RunInstall
    if ($result.code -ne 0 -or $result.text -notmatch 'running 1\.0\.0') { Fail "first install ($($result.code)):`n$($result.text)" }
    if (-not (Test-Path -LiteralPath $probe)) { Fail 'current does not point at the core' }
    $xml = TaskXml
    if (-not $xml) { Fail 'the logon task was not registered' }
    if ($xml -notmatch 'serve --headless') { Fail "the task does not run the core headless:`n$xml" }
    # schtasks prints the element only when the task is turned off.
    if ($xml -match '<Settings>[\s\S]*<Enabled>false</Enabled>') { Fail "the task is not enabled:`n$xml" }
    if ($xml -notmatch 'PT0S') { Fail "the task has a time limit:`n$xml" }
    if ((RunningVersion) -ne '1.0.0') { Fail "the task is not running 1.0.0" }
    if (-not (Test-Path -LiteralPath (Join-Path $prefix 'versions\1.0.0\LICENSE.txt') -PathType Leaf)) { Fail 'the license was not installed beside the core' }
    Pass 'first install registers the task and the core answers'

    $before = (Get-Item -LiteralPath (Join-Path $prefix 'versions\1.0.0')).LastWriteTimeUtc
    $result = RunInstall
    if ($result.code -ne 0 -or $result.text -notmatch 'already up to date') { Fail "rerun ($($result.code)):`n$($result.text)" }
    if ((Get-Item -LiteralPath (Join-Path $prefix 'versions\1.0.0')).LastWriteTimeUtc -ne $before) { Fail 'rerun rewrote the install' }
    Pass 'rerun is a no-op'

    Release '1.0.1' $zip101
    $result = RunInstall
    if ($result.code -ne 0 -or $result.text -notmatch 'running 1\.0\.1') { Fail "update ($($result.code)):`n$($result.text)" }
    if ((RunningVersion) -ne '1.0.1') { Fail 'the task is not running 1.0.1 after the update' }
    $kept = (Installed) -join ' '
    if ($kept -ne '1.0.0 1.0.1') { Fail "versions after the update: $kept" }
    Pass 'update switches the task to the new version and keeps the old one'

    $result = RunInstall @('-Rollback')
    if ($result.code -ne 0 -or $result.text -notmatch 'now running 1\.0\.0') { Fail "rollback ($($result.code)):`n$($result.text)" }
    if ((RunningVersion) -ne '1.0.0') { Fail 'the task is not running 1.0.0 after the rollback' }
    Pass 'rollback goes back to the previous version'

    # A second rollback never goes forward to the version just left.
    $result = RunInstall @('-Rollback')
    if ($result.code -eq 0 -or $result.text -notmatch 'no earlier version') { Fail "a second rollback went forward ($($result.code)):`n$($result.text)" }
    if ((RunningVersion) -ne '1.0.0') { Fail 'the refused rollback disturbed the running core' }
    Pass 'rollback only goes to older versions'

    $result = RunInstall @('-Version', '1.0.1')
    if ($result.code -ne 0 -or (RunningVersion) -ne '1.0.1') { Fail "reinstalling 1.0.1 ($($result.code)):`n$($result.text)" }

    # An older version that no longer starts: the rollback goes back to the
    # version that was running.
    Copy-Item -LiteralPath (Join-Path $root 'build-1.0.2\agent_relay\helpers\agent-relay-probe.exe') -Destination (Join-Path $prefix 'versions\1.0.0\agent-relay-probe.exe') -Force
    $result = RunInstall @('-Rollback') 12
    if ($result.code -eq 0 -or $result.text -notmatch '1\.0\.1 is running again') { Fail "a rollback to a core that never answers was kept ($($result.code)):`n$($result.text)" }
    if ((RunningVersion) -ne '1.0.1') { Fail 'the working version is not running after the failed rollback' }
    Pass 'a rollback to a version that never answers goes back to the working one'

    # An earlier release's core as the rollback target: it runs and answers
    # only in the `relay` form, so the task and the health check must use it.
    Copy-Item -LiteralPath (Join-Path $root 'build-1.0.0-earlier\agent_relay\helpers\agent-relay-probe.exe') -Destination (Join-Path $prefix 'versions\1.0.0\agent-relay-probe.exe') -Force
    $result = RunInstall @('-Rollback')
    if ($result.code -ne 0 -or $result.text -notmatch 'now running 1\.0\.0') { Fail "rollback to an earlier release's core ($($result.code)):`n$($result.text)" }
    if ((RunningVersion) -ne '1.0.0') { Fail "the earlier release's core does not answer relay call status.get after the rollback" }
    if ((TaskXml) -notmatch '<Arguments>relay serve --headless') { Fail "the task does not run the earlier release's core as relay serve --headless:`n$(TaskXml)" }
    Pass "rollback runs and asks an earlier release's core in the relay form"

    Release '1.0.2' $zip102
    $result = RunInstall @() 12
    if ($result.code -eq 0 -or $result.text -notmatch 'rolled back to 1\.0\.0') { Fail "a core that never answers was kept ($($result.code)):`n$($result.text)" }
    if ((RunningVersion) -ne '1.0.0') { Fail "the earlier release's core does not answer relay call status.get after the failed update" }
    if ((TaskXml) -notmatch '<Arguments>relay serve --headless') { Fail "the task does not run the earlier release's core as relay serve --headless after the failed update:`n$(TaskXml)" }
    if (Test-Path -LiteralPath (Join-Path $prefix 'versions\1.0.2')) { Fail 'the failed version was kept' }
    Pass "a new version that never answers is rolled back to the earlier release's core"

    $result = RunInstall @('-Version', '1.0.1')
    if ($result.code -ne 0 -or (RunningVersion) -ne '1.0.1') { Fail "reinstalling 1.0.1 over the earlier release's core ($($result.code)):`n$($result.text)" }
    if ((TaskXml) -notmatch '<Arguments>serve --headless') { Fail "the task still runs a later core in the relay form:`n$(TaskXml)" }

    Release '1.0.3' $zip101 -BadChecksum
    $result = RunInstall
    if ($result.code -eq 0 -or $result.text -notmatch 'does not match its published checksum') { Fail "bad checksum was accepted ($($result.code)):`n$($result.text)" }
    if (Test-Path -LiteralPath (Join-Path $prefix 'versions\1.0.3')) { Fail 'bad checksum left a version behind' }
    if ((RunningVersion) -ne '1.0.1') { Fail 'bad checksum disturbed the running core' }
    Pass 'bad checksum is refused'

    Release '1.0.3' $zip101
    $result = RunInstall
    if ($result.code -eq 0 -or $result.text -notmatch 'reports 1\.0\.1, not 1\.0\.3') { Fail "a mislabeled core was accepted ($($result.code)):`n$($result.text)" }
    if (Test-Path -LiteralPath (Join-Path $prefix 'versions\1.0.3')) { Fail 'a mislabeled core was left behind' }
    Pass 'a core reporting another version is refused'

    $result = RunInstall @('-Version', '1.0.1', '-DryRun')
    if ($result.code -ne 0 -or $result.text -notmatch 'already up to date') { Fail "dry run ($($result.code)):`n$($result.text)" }
    Pass 'dry run changes nothing'

    # A second run while one holds the install lock is refused.
    $held = [IO.File]::Open((Join-Path $prefix 'install.lock'), [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    try {
        $result = RunInstall
        if ($result.code -eq 0 -or $result.text -notmatch 'another install is running') { Fail "an overlapping run was not refused ($($result.code)):`n$($result.text)" }
    } finally { $held.Dispose() }
    Pass 'an overlapping run is refused'

    StopTask
    & schtasks.exe /Create /TN $task /TR '"C:\Users\a\AppData\Local\Programs\AgentRelay\1.0.0\bin\agent-relay.exe" --headless' /SC ONLOGON /F | Out-Null
    $result = RunInstall
    if ($result.code -ne 3 -or $result.text -notmatch 'owns the') { Fail "the app's task was not left alone ($($result.code)):`n$($result.text)" }
    Pass "the app's task is left to the app"

    # --- a site profile's copy on the cloud release feed ---
    . (Join-Path $PSScriptRoot 'test-windows-install-cloud.ps1')

    # --- the real core, from the build, on a socket and data directory of its own ---
    if ($ProbeZip) {
        Reset
        Release $Version $ProbeZip
        $result = RunInstall @() 90
        if ($result.code -ne 0 -or $result.text -notmatch "running $([regex]::Escape($Version))") { Fail "real core install ($($result.code)):`n$($result.text)" }
        $running = RunningVersion
        if ($running -ne $Version) { Fail "the real core reports '$running', not $Version" }
        if ((TaskXml) -notmatch [regex]::Escape('--app-data-dir')) { Fail 'the task does not carry the isolated data directory' }
        Pass 'the real core installs and answers from the task'
    } else {
        Write-Host 'skip - no -ProbeZip; the real core is not exercised'
    }
} finally {
    Cleanup
}
Write-Host 'all install tests passed'
