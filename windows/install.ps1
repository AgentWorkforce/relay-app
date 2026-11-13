<#
.SYNOPSIS
install.ps1 -- install or update Agent Relay's headless core on Windows.

.DESCRIPTION
One idempotent command for a server or a PC without the app, the Windows
counterpart of linux/install.sh: it installs the bare core
(agent-relay-probe.exe and the DLLs it needs) for this account, runs it as the
"Agent Relay (<account>)" logon task in Task Scheduler, and on later runs
updates it in place. Run it again at any time; an up-to-date healthy install is
left alone. No administrator rights are needed.

  powershell -ExecutionPolicy Bypass -File install.ps1 [-Version X.Y.Z] [-Rollback] [-DryRun]
  powershell -ExecutionPolicy Bypass -Command "irm https://raw.githubusercontent.com/AgentWorkforce/relay-app/main/windows/install.ps1 | iex"

Run it in a shell of its own, as above: the script ends with `exit`, which in
an interactive shell closes that shell.

What it does:
  1. Finds what is installed: a per-user install from this script
     (%LOCALAPPDATA%\Programs\AgentRelay\core\current), or the app, whose
     logon task runs agent-relay.exe (the app owns the task; this script then
     stops).
  2. Resolves the release (latest, or -Version) from the published releases,
     downloads <AssetPrefix>-<version>-Windows-<arch>-probe.zip and verifies
     it against the release's .sha256. The checksum proves the download is
     the published file, not who published it: Windows releases carry no
     signature yet. A site profile with $ReleaseFeed = 'cloud' reads its
     site's signed release feed instead (Cloud's proposed contract, see "the
     cloud release feed" below): the manifest's signature, checked by the
     installed core's `feed verify` on an update and by OpenSSL 3 on PATH on
     a first install, then the asset's size and sha256. With neither, it
     stops and asks for OpenSSL 3, unless -AllowUnverifiedFeed or
     AGENT_RELAY_ALLOW_UNVERIFIED_FEED=1 accepts the skipped check (it then
     says so, loudly). A first install needs AGENT_RELAY_INSTALL_TOKEN, the
     install token from the workspace's downloads page; an update uses the
     installed core's sign-in.
  3. Unpacks into versions\<version>, checks the binary reports that version,
     records the product beside it (versions\<version>\product), and points
     `current` (a directory junction) at it. The previous version stays for
     rollback; older ones beyond KEEP are removed.
  4. Registers the logon task (only when it is missing or names another
     program) from the same definition the app uses, and restarts it (or
     enables and starts it on a first install). Ending the task ends the
     core. A spawned Claude Code reads its input from the core and ends
     with it, so finish or hand off that work first; a spawned Codex runs
     on Codex's app server, which the core starts outside its own job, and
     keeps running, as agents in their own systemd scopes do on Linux: the
     restarted core picks it up again.
  5. Waits for the new core to answer with the new version; if it does not,
     points `current` back at the previous version and restarts again.

Every product installs into the same prefix and task: one Agent Relay app
per account, so a site profile's copy of this script replaces whichever
product's headless core is installed, as an update would, even at the same
version: a release counts as installed only when the recorded product is
this one (an install without a record is Agent Relay's).

Never touched: settings, sign-in and agent registrations, which live in the
core's data directory (%USERPROFILE%\.local\share\com.agentrelay.desktop),
not here. Never done: anything needing administrator rights, or changing
credentials.

Overrides, for tests and mirrors: AGENT_RELAY_RELEASES_API,
AGENT_RELAY_DOWNLOAD_BASE (an https address or a local folder; both are
refused by a copy that reads the cloud release feed),
AGENT_RELAY_PREFIX, AGENT_RELAY_TASK, AGENT_RELAY_SERVE_ARGS (appended to
`serve --headless`, or `relay serve --headless` for an earlier release's
core), AGENT_RELAY_CORE_SOCKET (the socket the health
check asks) and AGENT_RELAY_HEALTH_TIMEOUT.
#>
[CmdletBinding()]
param(
    # Install this release instead of the latest.
    [string]$Version = '',
    # Switch back to the previously installed version.
    [switch]$Rollback,
    # Say what would change and change nothing.
    [switch]$DryRun,
    # A cloud release feed only: install even when the manifest's signature
    # cannot be checked (no installed core with feed verify, no OpenSSL 3);
    # AGENT_RELAY_ALLOW_UNVERIFIED_FEED=1 does the same.
    [switch]$AllowUnverifiedFeed
)

Set-StrictMode -Version 2
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

# The product whose releases this installs. A site profile's release
# publishes a copy with these lines set to its own (Stage the release
# assets, .github/workflows/desktop-release.yml); keep each one assignment.
# ReleaseFeed is 'github' (the releases repository) or 'cloud' (the signed
# release feed of SiteUrl for ProfileId, checked with FeedPublicKey, the
# standard base64 of the feed's Ed25519 public key).
$ProductName = 'Agent Relay'
$ReleasesRepository = 'AgentWorkforce/relay-desktop-releases'
$AssetPrefix = 'AgentRelay'
$ReleaseFeed = 'github'
$SiteUrl = ''
$ProfileId = ''
$FeedPublicKey = ''
# The product an install without a product record belongs to: every earlier
# release of this script was Agent Relay's.
$DefaultAssetPrefix = 'AgentRelay'

$ScriptName = 'agent-relay install'
$ReleasesApi = if ($env:AGENT_RELAY_RELEASES_API) { $env:AGENT_RELAY_RELEASES_API } else { "https://api.github.com/repos/$ReleasesRepository/releases" }
$DownloadBase = if ($env:AGENT_RELAY_DOWNLOAD_BASE) { $env:AGENT_RELAY_DOWNLOAD_BASE } else { "https://github.com/$ReleasesRepository/releases/download" }
$Prefix = if ($env:AGENT_RELAY_PREFIX) { $env:AGENT_RELAY_PREFIX } else { Join-Path $env:LOCALAPPDATA 'Programs\AgentRelay\core' }
$Keep = 3
$HealthTimeout = if ($env:AGENT_RELAY_HEALTH_TIMEOUT) { [int]$env:AGENT_RELAY_HEALTH_TIMEOUT } else { 60 }
$Probe = 'agent-relay-probe.exe'
$Current = Join-Path $Prefix 'current'
$Versions = Join-Path $Prefix 'versions'
# The core writes its own log once it runs; one that fails before that has
# no console under the task, and running it by hand shows why.
$Log = "see $env:USERPROFILE\.local\share\com.agentrelay.desktop\logs\core.log, or run & `"$Current\$Probe`" serve --headless in PowerShell to see why"

# The task's name is the app's: "Agent Relay (alice)", one per account, so
# the app and this script never run two cores for one person.
$Account = if ($env:USERNAME) { ($env:USERNAME -split '\\')[-1] } else { '' }
$Task = if ($env:AGENT_RELAY_TASK) { $env:AGENT_RELAY_TASK } elseif ($Account) { "Agent Relay ($Account)" } else { 'Agent Relay' }
$TaskUser = if ($env:USERDOMAIN -and $env:USERNAME) { "$env:USERDOMAIN\$env:USERNAME" } else { $env:USERNAME }

# The per-prefix lock, taken below and released by Finish: `irm | iex` runs
# the script inside a shell that lives on, so the handle must not wait for
# the process to end.
$script:InstallLock = $null

function Finish([int]$Code) {
    if ($script:InstallLock) { $script:InstallLock.Dispose(); $script:InstallLock = $null }
    exit $Code
}
function Note([string]$Text) { Write-Host "${ScriptName}: $Text" }
function Die([string]$Text) { Write-Host "${ScriptName}: error: $Text" -ForegroundColor Red; Finish 1 }
function WouldRun([string]$Text) { Write-Host "${ScriptName}: would $Text" }

if (-not $env:LOCALAPPDATA -and -not $env:AGENT_RELAY_PREFIX) { Die 'LOCALAPPDATA is not set' }
if (-not $env:USERPROFILE) { Die 'USERPROFILE is not set' }

$Arch = switch ($env:PROCESSOR_ARCHITECTURE) {
    'ARM64' { 'arm64' }
    'AMD64' { 'x64' }
    default {
        # A 32-bit PowerShell on a 64-bit Windows reports the emulated architecture.
        if ($env:PROCESSOR_ARCHITEW6432 -eq 'ARM64') { 'arm64' }
        elseif ($env:PROCESSOR_ARCHITEW6432 -eq 'AMD64') { 'x64' }
        else { Die "unsupported architecture $env:PROCESSOR_ARCHITECTURE" }
    }
}

function Test-ReleaseVersion([string]$Candidate) {
    return $Candidate -match '^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$'
}
$Version = $Version -replace '^v', ''
if ($Version -and -not (Test-ReleaseVersion $Version)) { Die "not a release version: $Version" }

if (-not (Get-Command schtasks.exe -ErrorAction SilentlyContinue)) {
    Die 'schtasks.exe is required: the core runs as a Task Scheduler logon task'
}

# --- helpers ------------------------------------------------------------------

# Fetches a release file: from the web, or from a local folder when the base
# is one (tests, mirrors).
function Fetch([string]$Base, [string]$Relative, [string]$Destination) {
    if (Test-Path -LiteralPath $Base -PathType Container) {
        $source = Join-Path $Base ($Relative -replace '/', '\')
        if (-not (Test-Path -LiteralPath $source -PathType Leaf)) { throw "missing $source" }
        Copy-Item -LiteralPath $source -Destination $Destination -Force
        return
    }
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    $url = "$($Base.TrimEnd('/'))/$Relative"
    $attempt = 0
    while ($true) {
        try {
            Invoke-WebRequest -Uri $url -OutFile $Destination -UseBasicParsing -Headers @{ 'User-Agent' = 'agent-relay-install' }
            return
        } catch {
            $attempt++
            if ($attempt -ge 3) { throw }
            Start-Sleep -Seconds (2 * $attempt)
        }
    }
}

function Sha256Of([string]$Path) {
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

# The version an executable reports with --version, or nothing.
function ProbeVersion([string]$Exe) {
    # Windows PowerShell turns a program's redirected stderr into errors.
    $ErrorActionPreference = 'Continue'
    try {
        $output = & $Exe --version 2>$null
        if ($LASTEXITCODE -ne 0 -or -not $output) { return '' }
        $line = ($output | Select-Object -First 1).ToString().Trim()
        return ($line -split '\s+')[-1]
    } catch { return '' }
}

# What the task runs $Exe with. Every earlier release's core needs the
# `relay` word before `serve`; a later one takes `serve` alone, and given the
# word runs itself again as a second process, which ending the task would
# leave behind. AGENT_RELAY_SERVE_ARGS (tests, mirrors) appends `serve`
# options such as --socket and --app-data-dir; a client in the same
# environment reaches that socket through AGENT_RELAY_CORE_SOCKET, which
# `call` honors.
function ServeArguments([string]$Exe = (Join-Path $Current $Probe)) {
    $ErrorActionPreference = 'Continue'
    $serve = 'serve'
    if (Test-Path -LiteralPath $Exe -PathType Leaf) {
        try {
            $null = & $Exe serve --help 2>$null
            if ($LASTEXITCODE -ne 0) { $serve = 'relay serve' }
        } catch { $serve = 'relay serve' }
    }
    return ("$serve --headless " + [string]$env:AGENT_RELAY_SERVE_ARGS).Trim()
}

# What the running core reports, or nothing. `relay call` is the form every
# release's core accepts, earlier ones included, so a rollback is checked too.
function CoreVersion([string]$Exe = (Join-Path $Current $Probe)) {
    $ErrorActionPreference = 'Continue'
    try {
        $output = & $Exe relay call status.get 2>$null
        if ($LASTEXITCODE -ne 0 -or -not $output) { return '' }
        $json = ($output -join "`n") | ConvertFrom-Json
        $daemon = $null
        if ($json.PSObject.Properties['result'] -and $json.result.PSObject.Properties['daemon']) { $daemon = $json.result.daemon }
        elseif ($json.PSObject.Properties['daemon']) { $daemon = $json.daemon }
        if ($daemon -and $daemon.PSObject.Properties['version']) { return [string]$daemon.version }
        return ''
    } catch { return '' }
}

# Healthy means the task is running and the core answering is the wanted
# version: a core someone started by hand (the troubleshooting command) can
# hold the socket and answer while the task's own core exited, which would
# otherwise pass for a working install until that terminal closes.
function Healthy([string]$Wanted, [string]$Exe = (Join-Path $Current $Probe)) {
    if ($DryRun) { return $true }
    $waited = 0
    while ($waited -lt $HealthTimeout) {
        if ((CoreVersion $Exe) -eq $Wanted -and (TaskRunning)) { return $true }
        Start-Sleep -Seconds 2
        $waited += 2
    }
    return $false
}

# Installed versions, oldest first.
function InstalledVersions {
    if (-not (Test-Path -LiteralPath $Versions)) { return @() }
    return @(Get-ChildItem -LiteralPath $Versions -Directory | Where-Object { $_.Name -notlike '*.partial' -and $_.Name -notlike '*.replaced' } |
        Sort-Object { [Version](($_.Name -split '[-+]')[0]) }, Name | ForEach-Object { $_.Name })
}

# Where `current` points, or nothing.
function CurrentTarget {
    if (-not (Test-Path -LiteralPath $Current)) { return '' }
    $item = Get-Item -LiteralPath $Current -Force
    if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) {
        $target = $item.Target
        if ($target -is [array]) { $target = $target[0] }
        return [string]$target
    }
    return ''
}

function SwitchTo([string]$To) {
    $target = Join-Path $Versions $To
    if ($DryRun) { WouldRun "point $Current at $target"; return }
    if (Test-Path -LiteralPath $Current) {
        # Removes the junction only, never what it points at.
        [IO.Directory]::Delete($Current)
    }
    New-Item -ItemType Junction -Path $Current -Target $target | Out-Null
}

# --- the logon task -------------------------------------------------------------

# --- the cloud release feed ($ReleaseFeed = 'cloud') ---------------------------
#
# The routes, headers and error codes below are Cloud's proposed release feed
# contract (docs/enterprise-distribution.md), not a stable API. The manifest
# is a signed envelope {manifest, signature, key_id}; this script refuses one
# whose key_id is not fk_<ProfileId>_<label>, whose profile or channel is not
# this build's, or whose signature does not verify, then checks the asset's
# size and sha256 before anything is unpacked.
#
# The credential is never printed, logged or written to a file: it is held
# in memory and handed to the HTTP client as a header, and only on requests
# to the site itself.

$Site = $SiteUrl.TrimEnd('/')
$FeedLimit = 1048576
$FeedVersion = '^[0-9]{4}\.(1[0-2]|[1-9])\.(0|[1-9][0-9]*)\z'
$FeedHost = '^(?=.{1,253}\z)([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]([a-z0-9-]{0,61}[a-z0-9])?\z'
$script:FeedHeaders = $null
$script:FeedErrorBody = ''
$script:Cloud = $null

function CloudPreflight {
    foreach ($name in 'AGENT_RELAY_RELEASES_API', 'AGENT_RELAY_DOWNLOAD_BASE') {
        if ($null -ne [Environment]::GetEnvironmentVariable($name)) { Die "$name cannot be used: $ProductName installs from its site's release feed" }
    }
    if (-not $SiteUrl -or -not $ProfileId -or -not $FeedPublicKey) {
        Die "this copy of install.ps1 has no site, profile or feed key; download it again from your workspace's downloads page"
    }
    if ($Site -cnotmatch '^https://[a-z0-9]([a-z0-9.-]*[a-z0-9])?\z') { Die 'the built-in site is not an https:// address' }
    if ($ProfileId -cnotmatch '^[a-z0-9]{1,32}\z') { Die 'the built-in profile id is malformed' }
}

# A strict JSON reader for the feed's answers: objects (a repeated key is
# refused), arrays, strings, true, false, null and integers within
# +-(2^53-1); fractions and exponents are refused. Objects come back as
# ordered dictionaries, arrays as lists, integers as [long].
function ReadJson([string]$Text) {
    $script:JsonText = $Text
    $script:JsonAt = 0
    $value = JsonValue 0
    JsonSpace
    if ($script:JsonAt -ne $Text.Length) { throw 'trailing data after the JSON value' }
    return , $value
}

function ReadJsonFile([string]$Path) {
    if ((Get-Item -LiteralPath $Path).Length -gt $FeedLimit) { throw 'the answer is larger than 1 MiB' }
    return , (ReadJson ([IO.File]::ReadAllText($Path, [Text.UTF8Encoding]::new($false, $true))))
}

function JsonSpace {
    while ($script:JsonAt -lt $script:JsonText.Length -and " `t`r`n".IndexOf($script:JsonText[$script:JsonAt]) -ge 0) { $script:JsonAt++ }
}

function JsonExpect([string]$Character) {
    JsonSpace
    if ($script:JsonAt -ge $script:JsonText.Length -or [string]$script:JsonText[$script:JsonAt] -cne $Character) { throw "expected $Character" }
    $script:JsonAt++
}

function JsonNext {
    JsonSpace
    if ($script:JsonAt -ge $script:JsonText.Length) { throw 'unexpected end of the JSON' }
    return [string]$script:JsonText[$script:JsonAt]
}

function JsonValue([int]$Depth) {
    if ($Depth -gt 32) { throw 'the JSON is nested too deeply' }
    $next = JsonNext
    $text = $script:JsonText
    if ($next -ceq '{') {
        $script:JsonAt++
        $object = [Collections.Specialized.OrderedDictionary]::new()
        if ((JsonNext) -ceq '}') { $script:JsonAt++; return $object }
        while ($true) {
            if ((JsonNext) -cne '"') { throw 'expected a key' }
            $key = JsonString
            if ($object.Contains($key)) { throw 'a repeated key' }
            JsonExpect ':'
            $object[$key] = JsonValue ($Depth + 1)
            $next = JsonNext
            $script:JsonAt++
            if ($next -ceq '}') { return $object }
            if ($next -cne ',') { throw 'expected , or }' }
        }
    }
    if ($next -ceq '[') {
        $script:JsonAt++
        $list = [Collections.Generic.List[object]]::new()
        if ((JsonNext) -ceq ']') { $script:JsonAt++; return , $list }
        while ($true) {
            $list.Add((JsonValue ($Depth + 1)))
            $next = JsonNext
            $script:JsonAt++
            if ($next -ceq ']') { return , $list }
            if ($next -cne ',') { throw 'expected , or ]' }
        }
    }
    if ($next -ceq '"') { return JsonString }
    foreach ($word in 'true', 'false', 'null') {
        if ($text.Length - $script:JsonAt -ge $word.Length -and [string]::CompareOrdinal($text, $script:JsonAt, $word, 0, $word.Length) -eq 0) {
            $script:JsonAt += $word.Length
            if ($word -ceq 'true') { return $true }
            if ($word -ceq 'false') { return $false }
            return $null
        }
    }
    $number = [regex]::new('\G-?(0|[1-9][0-9]{0,15})').Match($text, $script:JsonAt)
    if (-not $number.Success) { throw 'not a JSON value' }
    $script:JsonAt += $number.Length
    if ($script:JsonAt -lt $text.Length -and '0123456789.eE'.IndexOf($text[$script:JsonAt]) -ge 0) { throw 'a number that is not a safe integer' }
    $integer = [long]::Parse($number.Value, [Globalization.CultureInfo]::InvariantCulture)
    if ([Math]::Abs($integer) -gt 9007199254740991) { throw 'a number that is not a safe integer' }
    return $integer
}

function JsonString {
    $text = $script:JsonText
    $script:JsonAt++
    $out = [Text.StringBuilder]::new()
    while ($true) {
        if ($script:JsonAt -ge $text.Length) { throw 'an unterminated string' }
        $character = $text[$script:JsonAt]
        $script:JsonAt++
        if ([int]$character -lt 0x20) { throw 'a control character in a string' }
        if ([string]$character -ceq '"') { return $out.ToString() }
        if ([string]$character -cne '\') { [void]$out.Append($character); continue }
        if ($script:JsonAt -ge $text.Length) { throw 'an unterminated string' }
        $escape = [string]$text[$script:JsonAt]
        $script:JsonAt++
        if ($escape -ceq 'u') {
            $hex = if ($script:JsonAt + 4 -le $text.Length) { $text.Substring($script:JsonAt, 4) } else { '' }
            if ($hex -cnotmatch '^[0-9A-Fa-f]{4}\z') { throw 'a bad \u escape' }
            [void]$out.Append([char][Convert]::ToInt32($hex, 16))
            $script:JsonAt += 4
            continue
        }
        $index = '"\/bfnrt'.IndexOf([char]$escape)
        if ($index -lt 0) { throw 'a bad escape' }
        [void]$out.Append(@('"', '\', '/', [string][char]8, [string][char]12, "`n", "`r", "`t")[$index])
    }
}

# RFC 8785 (JCS) for what ReadJson returns: keys in UTF-16 order, strings
# with only the escapes JSON requires.
function Jcs($Value) {
    if ($null -eq $Value) { return 'null' }
    if ($Value -is [bool]) { if ($Value) { return 'true' } else { return 'false' } }
    if ($Value -is [long]) { return $Value.ToString([Globalization.CultureInfo]::InvariantCulture) }
    if ($Value -is [string]) { return JcsString $Value }
    if ($Value -is [Collections.Specialized.OrderedDictionary]) {
        [string[]]$keys = @($Value.Keys)
        [Array]::Sort($keys, [StringComparer]::Ordinal)
        $parts = @(foreach ($key in $keys) { (JcsString $key) + ':' + (Jcs $Value[$key]) })
        return '{' + ($parts -join ',') + '}'
    }
    if ($Value -is [Collections.Generic.List[object]]) {
        $parts = @(foreach ($item in $Value) { Jcs $item })
        return '[' + ($parts -join ',') + ']'
    }
    throw 'not a JSON value'
}

function JcsString([string]$Text) {
    $out = [Text.StringBuilder]::new()
    [void]$out.Append('"')
    foreach ($character in $Text.ToCharArray()) {
        $code = [int]$character
        if ($code -eq 0x22) { [void]$out.Append('\"') }
        elseif ($code -eq 0x5c) { [void]$out.Append('\\') }
        elseif ($code -eq 8) { [void]$out.Append('\b') }
        elseif ($code -eq 9) { [void]$out.Append('\t') }
        elseif ($code -eq 10) { [void]$out.Append('\n') }
        elseif ($code -eq 12) { [void]$out.Append('\f') }
        elseif ($code -eq 13) { [void]$out.Append('\r') }
        elseif ($code -lt 0x20) { [void]$out.Append(('\u{0:x4}' -f $code)) }
        else { [void]$out.Append($character) }
    }
    [void]$out.Append('"')
    return $out.ToString()
}

function IsText($Value, [string]$Pattern) { return ($Value -is [string]) -and ($Value -cmatch $Pattern) }

# FeedExchange url method headers destination limit: one HTTP exchange,
# never following a redirect. A 200 answer is copied to Destination, at most
# Limit bytes of it: one whose Content-Length is larger is refused before
# its body is read, and one that runs past Limit is cut off, removed and
# refused. Returns @{ status; body }: status -1 for an answer larger than
# Limit, body the start (64 KiB at most) of any other status's answer. A
# failed exchange throws.
function FeedExchange([string]$Url, [string]$Method, [hashtable]$Headers, [string]$Destination, [long]$Limit) {
    Add-Type -AssemblyName System.Net.Http
    $handler = [System.Net.Http.HttpClientHandler]::new()
    $handler.AllowAutoRedirect = $false
    $handler.UseCookies = $false
    $client = [System.Net.Http.HttpClient]::new($handler)
    $client.Timeout = [TimeSpan]::FromMinutes(30)
    $request = $null; $response = $null; $source = $null; $sink = $null; $complete = $false; $status = 0
    try {
        $verb = if ($Method -eq 'Post') { [System.Net.Http.HttpMethod]::Post } else { [System.Net.Http.HttpMethod]::Get }
        $request = [System.Net.Http.HttpRequestMessage]::new($verb, [Uri]$Url)
        foreach ($name in $Headers.Keys) { [void]$request.Headers.TryAddWithoutValidation([string]$name, [string]$Headers[$name]) }
        if ($Method -eq 'Post') { $request.Content = [System.Net.Http.ByteArrayContent]::new([byte[]]::new(0)) }
        $response = $client.SendAsync($request, [System.Net.Http.HttpCompletionOption]::ResponseHeadersRead).GetAwaiter().GetResult()
        $status = [int]$response.StatusCode
        $cap = if ($status -eq 200) { $Limit } else { 65536 }
        $length = $response.Content.Headers.ContentLength
        if ($status -eq 200 -and $null -ne $length -and $length -gt $Limit) { return @{ status = -1; body = '' } }
        $source = $response.Content.ReadAsStreamAsync().GetAwaiter().GetResult()
        if ($status -eq 200) {
            $sink = [IO.File]::Open($Destination, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
        } else {
            $sink = [IO.MemoryStream]::new()
        }
        $buffer = [byte[]]::new(81920)
        [long]$total = 0
        while (($read = $source.Read($buffer, 0, $buffer.Length)) -gt 0) {
            $keep = [long][Math]::Min([long]$read, $cap - $total)
            if ($keep -gt 0) { $sink.Write($buffer, 0, [int]$keep) }
            $total += $read
            if ($total -gt $cap) { break }
        }
        if ($status -eq 200) {
            if ($total -gt $Limit) { return @{ status = -1; body = '' } }
            $complete = $true
            return @{ status = 200; body = '' }
        }
        return @{ status = $status; body = [Text.Encoding]::UTF8.GetString($sink.ToArray()) }
    } finally {
        foreach ($disposable in $sink, $source, $response, $request, $client) { if ($disposable) { $disposable.Dispose() } }
        if ($status -eq 200 -and -not $complete) { Remove-Item -LiteralPath $Destination -Force -ErrorAction SilentlyContinue }
    }
}

# FeedRequest url destination limit [-Method Post] [-WithCredential]: one
# request, the credential only with -WithCredential, never a redirect, the
# answer capped at Limit bytes (FeedExchange). Returns the HTTP status, -1
# when the answer is larger than Limit, or 0 when nothing usable came back;
# the error body, if any, is kept in $script:FeedErrorBody.
function FeedRequest([string]$Url, [string]$Destination, [long]$Limit, [string]$Method = 'Get', [switch]$WithCredential) {
    $script:FeedErrorBody = ''
    if ($Url -cnotmatch '^https://') { return 0 }
    $headers = @{ 'User-Agent' = 'agent-relay-install' }
    if ($WithCredential) { foreach ($name in $script:FeedHeaders.Keys) { $headers[$name] = $script:FeedHeaders[$name] } }
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12
    $attempt = 0
    while ($true) {
        Remove-Item -LiteralPath $Destination -Force -ErrorAction SilentlyContinue
        $answer = $null
        try { $answer = FeedExchange $Url $Method $headers $Destination $Limit } catch { $answer = $null }
        if ($answer -is [hashtable]) {
            $status = [int]$answer['status']
            if ($status -eq 200 -and -not (Test-Path -LiteralPath $Destination -PathType Leaf)) { return 0 }
            if ($status -ne 200 -and $status -gt 0) { $script:FeedErrorBody = [string]$answer['body'] }
            return $status
        }
        $attempt++
        if ($attempt -ge 3) { return 0 }
        Start-Sleep -Seconds (2 * $attempt)
    }
}

# The installed core, if it can supply headers and verify updates.
function InstalledProbe {
    $exe = Join-Path $Current $Probe
    if ($currentVersion -and (Test-Path -LiteralPath $exe -PathType Leaf)) { return $exe }
    return ''
}

# The installed core's workspace and account, or nothing when it is not
# signed in.
function CloudIds([string]$Exe) {
    $ErrorActionPreference = 'Continue'
    try {
        $output = & $Exe relay call status.get 2>$null
        if ($LASTEXITCODE -ne 0 -or -not $output) { return $null }
        $json = ($output -join "`n") | ConvertFrom-Json
        if ($json.PSObject.Properties['result']) { $json = $json.result }
        if (-not $json.PSObject.Properties['account'] -or -not $json.account) { return $null }
        $account = $json.account
        if (-not $account.PSObject.Properties['workspace_id'] -or -not $account.PSObject.Properties['person'] -or
            -not $account.person -or -not $account.person.PSObject.Properties['id']) { return $null }
        $workspace = [string]$account.workspace_id
        $who = [string]$account.person.id
        if ($workspace -cnotmatch '^[A-Za-z0-9_.-]{1,128}\z' -or $who -cnotmatch '^[A-Za-z0-9_.-]{1,128}\z') { return $null }
        return @($workspace, $who)
    } catch { return $null }
}

# Sets the feed headers: the install token when there is one, else the
# installed core's own (feed-headers), never both: Cloud refuses mixed
# credentials.
function CloudAuth {
    $token = [string]$env:AGENT_RELAY_INSTALL_TOKEN
    if ($token) {
        if ($token -cnotmatch '^[!-~]+\z') { Die "AGENT_RELAY_INSTALL_TOKEN is not an install token; copy it again from your workspace's downloads page" }
        $script:FeedHeaders = @{ 'Authorization' = "Bearer $token"; 'X-Agent-Relay-Profile' = $ProfileId }
        return
    }
    $exe = InstalledProbe
    if (-not $exe) {
        Die "a first install needs AGENT_RELAY_INSTALL_TOKEN: copy the install token from your workspace's downloads page and run this again with it set"
    }
    $ids = CloudIds $exe
    if (-not $ids) { Die "the installed core is not signed in to a workspace; sign in, or set AGENT_RELAY_INSTALL_TOKEN from your workspace's downloads page" }
    $answer = $null
    try {
        $ErrorActionPreference = 'Continue'
        $output = & $exe feed-headers --site $Site --workspace $ids[0] --account $ids[1] --profile $ProfileId 2>$null
        if ($LASTEXITCODE -eq 0 -and $output) { $answer = ReadJson (($output | ForEach-Object { $_.ToString() }) -join "`n") }
    } catch { $answer = $null } finally { $ErrorActionPreference = 'Stop' }
    if ($null -eq $answer) { Die "the installed core could not supply release feed headers; or set AGENT_RELAY_INSTALL_TOKEN from your workspace's downloads page" }
    if (-not ($answer -is [Collections.Specialized.OrderedDictionary]) -or $answer.Count -ne 3 -or
        -not (IsText $answer['Authorization'] '^Bearer [!-~]+\z') -or
        $answer['X-Agent-Relay-Profile'] -cne $ProfileId -or $answer['X-Agent-Relay-Desktop'] -cne '1') {
        Die 'the installed core supplied unusable release feed headers'
    }
    $script:FeedHeaders = @{}
    foreach ($name in 'Authorization', 'X-Agent-Relay-Profile', 'X-Agent-Relay-Desktop') { $script:FeedHeaders[$name] = $answer[$name] }
}

# Says why the feed refused, naming only its error code, never the body.
function CloudRefused([string]$What, [int]$Status) {
    $code = ''
    try {
        $body = ReadJson $script:FeedErrorBody
        if ($body -is [Collections.Specialized.OrderedDictionary] -and (IsText $body['error'] '^[a-z_]{1,64}\z')) { $code = $body['error'] }
    } catch { $code = '' }
    function CodeOr([string]$Fallback) { if ($code) { return $code } return $Fallback }
    switch ($Status) {
        401 {
            if (-not $env:AGENT_RELAY_INSTALL_TOKEN) {
                Die "the release feed refused the installed core's sign-in ($(CodeOr 'unauthenticated')); sign in again, or set AGENT_RELAY_INSTALL_TOKEN from your workspace's downloads page"
            }
            Die "the release feed refused the install token ($(CodeOr 'unauthenticated')): it has expired or was revoked; get a new one from your workspace's downloads page"
        }
        403 {
            if ($code -ceq 'not_entitled') { Die "this workspace is not entitled to $ProductName (not_entitled); ask a workspace admin" }
            Die "the release feed refused this request ($(CodeOr 'forbidden'))"
        }
        404 { Die "no $ProductName release is published on the stable channel ($(CodeOr 'no_release'))" }
        429 { Die "the release feed is busy ($(CodeOr 'rate_limited')); try again later" }
        0 { Die "could not read $What from $Site" }
        { $_ -lt 0 } { Die "$What from $Site is larger than 1 MiB" }
        default { Die "the release feed answered HTTP $Status for $What ($(CodeOr 'no error code'))" }
    }
}

# Checks the envelope's shape and key_id; writes the signature bytes and the
# manifest's JCS bytes for OpenSSL. Returns the manifest.
function CloudEnvelope([string]$Path, [string]$Work) {
    try { $envelope = ReadJsonFile $Path } catch { Die "the release feed's manifest was refused (schema_invalid)" }
    if (-not ($envelope -is [Collections.Specialized.OrderedDictionary]) -or $envelope.Count -ne 3 -or
        -not ($envelope['manifest'] -is [Collections.Specialized.OrderedDictionary]) -or
        -not (IsText $envelope['signature'] '^[A-Za-z0-9_-]{85}[AQgw]\z') -or
        -not (IsText $envelope['key_id'] '^fk_[a-z0-9]{1,32}_[a-z0-9]{1,16}\z')) {
        Die "the release feed answered something that is not a signed manifest (schema_invalid)"
    }
    $keyId = [string]$envelope['key_id']
    if ($keyId.Split('_')[1] -cne $ProfileId) { Die "the manifest is signed with $keyId, not a key of profile $ProfileId (key_id_refused)" }
    $signature = [string]$envelope['signature']
    $signature = $signature.Replace('-', '+').Replace('_', '/') + '=='
    [IO.File]::WriteAllBytes((Join-Path $Work 'manifest.sig'), [Convert]::FromBase64String($signature))
    try {
        $canonical = [Text.UTF8Encoding]::new($false, $true).GetBytes((Jcs $envelope['manifest']))
    } catch { Die "the release feed's manifest was refused (schema_invalid)" }
    [IO.File]::WriteAllBytes((Join-Path $Work 'manifest.jcs'), $canonical)
    return $envelope['manifest']
}

# Verifies the envelope's signature: with the installed core's feed verify
# for an update, else OpenSSL 3 when it is on PATH, else refuses, as
# linux/install.sh does, unless -AllowUnverifiedFeed or
# AGENT_RELAY_ALLOW_UNVERIFIED_FEED=1 accepts a skipped check, which is then
# said. Returns what verified it ('' when skipped); sets
# $script:CloudCurrent when the installed version is not older.
function CloudVerify([string]$Path, [string]$Work) {
    $exe = InstalledProbe
    if ($exe) {
        $ErrorActionPreference = 'Continue'
        $null = & $exe feed --help 2>$null
        $hasVerify = $LASTEXITCODE -eq 0
        if ($hasVerify) {
            $arguments = @('feed', 'verify', '--profile', $ProfileId, '--public-key', $FeedPublicKey)
            if ($currentProduct -ceq $AssetPrefix -and $currentVersion -cmatch $FeedVersion) { $arguments += @('--installed-version', $currentVersion) }
            $output = & $exe @arguments $Path 2>$null
            $verifyExit = $LASTEXITCODE
            $ErrorActionPreference = 'Stop'
            $result = $null
            try { $result = ReadJson ((@($output) | Select-Object -First 1 | ForEach-Object { $_.ToString() }) -join '') } catch { $result = $null }
            if (-not ($result -is [Collections.Specialized.OrderedDictionary])) { Die 'the installed core could not check the manifest' }
            if ($result['ok'] -is [bool] -and $result['ok'] -and $verifyExit -eq 0) { return 'the installed core' }
            if (-not (IsText $result['code'] '^[a-z_]{1,64}\z')) { Die 'the installed core could not check the manifest' }
            if ($result['code'] -ceq 'not_newer') { $script:CloudCurrent = $true; return 'the installed core' }
            Die "the release feed's manifest was refused ($($result['code']))"
        }
        $ErrorActionPreference = 'Stop'
    }
    $openssl = Get-Command openssl -ErrorAction SilentlyContinue | Select-Object -First 1
    $opensslVersion = ''
    if ($openssl) {
        $ErrorActionPreference = 'Continue'
        try { $opensslVersion = (@(& $openssl version 2>$null) | Select-Object -First 1 | ForEach-Object { $_.ToString() }) -join '' } catch { $opensslVersion = '' }
        $ErrorActionPreference = 'Stop'
    }
    if ($opensslVersion -cmatch '^OpenSSL 3\.') {
        $key = $null
        try { $key = [Convert]::FromBase64String($FeedPublicKey) } catch { $key = $null }
        if ($null -eq $key -or $key.Length -ne 32) { Die 'the built-in feed key is unusable' }
        [byte[]]$der = [byte[]](0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00) + $key
        $pem = Join-Path $Work 'feed.pem'
        [IO.File]::WriteAllText($pem, "-----BEGIN PUBLIC KEY-----`n$([Convert]::ToBase64String($der))`n-----END PUBLIC KEY-----`n", [Text.Encoding]::ASCII)
        $ErrorActionPreference = 'Continue'
        $null = & $openssl pkeyutl -verify -rawin -pubin -inkey $pem -sigfile (Join-Path $Work 'manifest.sig') -in (Join-Path $Work 'manifest.jcs') 2>$null
        $verified = $LASTEXITCODE -eq 0
        $ErrorActionPreference = 'Stop'
        if (-not $verified) { Die "the release feed's manifest signature does not verify (signature_invalid)" }
        return 'OpenSSL'
    }
    if (-not $AllowUnverifiedFeed -and $env:AGENT_RELAY_ALLOW_UNVERIFIED_FEED -cne '1') {
        Die "cannot check the release feed's signature: no installed core with feed verify and no OpenSSL 3 on PATH; install OpenSSL 3 (openssl) and run this again"
    }
    Note 'warning: the manifest''s signature check was skipped (no installed core with feed verify and no OpenSSL 3 on PATH);'
    Note "warning: this install trusts TLS to $Site and the install token, then the asset's size and sha256"
    Note 'warning: allowed by -AllowUnverifiedFeed or AGENT_RELAY_ALLOW_UNVERIFIED_FEED=1; install OpenSSL 3 to check it'
    return ''
}

# The one asset named $Wanted in the manifest, checked, or Die.
function CloudSelect($Manifest, [string]$Wanted) {
    if (-not ($Manifest['schema'] -is [long]) -or $Manifest['schema'] -ne 1) { Die 'the manifest has a schema this script does not know' }
    if ($Manifest['profile'] -cne $ProfileId) { Die 'the manifest is for another profile' }
    if ($Manifest['channel'] -cne 'stable') { Die 'the manifest is for another channel' }
    $version = $Manifest['version']
    if (-not (IsText $version $FeedVersion)) { Die 'the manifest has no usable stable version' }
    $hosts = $Manifest['download_hosts']
    $assets = $Manifest['assets']
    if (-not ($hosts -is [Collections.Generic.List[object]]) -or -not ($assets -is [Collections.Generic.List[object]]) -or
        @($hosts | Where-Object { -not (IsText $_ $FeedHost) }).Count -gt 0 -or
        @($assets | Where-Object { -not ($_ -is [Collections.Specialized.OrderedDictionary]) }).Count -gt 0) {
        Die 'the manifest is malformed'
    }
    $name = $Wanted.Replace('@VERSION@', $version)
    $found = @($assets | Where-Object { $_['name'] -ceq $name })
    if ($found.Count -ne 1) { Die "the release has no $name" }
    $asset = $found[0]
    $size = $asset['size']
    if ($asset['platform'] -cne 'Windows' -or $asset['arch'] -cne $Arch -or $asset['kind'] -cne 'probe' -or
        -not (IsText $asset['id'] '^[A-Za-z0-9_-]{1,64}\z') -or -not ($size -is [long]) -or $size -le 0 -or
        $size -gt 1099511627776 -or -not (IsText $asset['sha256'] '^[0-9a-f]{64}\z')) {
        Die "the manifest describes $name wrongly"
    }
    return @{ version = $version; id = $asset['id']; size = $size; sha256 = $asset['sha256']; asset = $name; hosts = @($hosts) }
}

# Resolves $Version and the asset from the signed manifest.
function CloudResolve([string]$Work) {
    CloudAuth
    $path = Join-Path $Work 'envelope.json'
    $status = FeedRequest "$Site/cloud/api/v1/desktop/feed/manifest?channel=stable" $path $FeedLimit -WithCredential
    if ($status -ne 200) { CloudRefused 'the release manifest' $status }
    $manifest = CloudEnvelope $path $Work
    $script:CloudCurrent = $false
    $verifiedBy = CloudVerify $path $Work
    $script:Cloud = CloudSelect $manifest "$AssetPrefix-@VERSION@-Windows-$Arch-probe.zip"
    if ($verifiedBy) { Note "release manifest verified by $verifiedBy" }
    if ($script:Version -and $script:Version -ne $script:Cloud.version) {
        Die "the release feed offers $($script:Cloud.version) only; -Version $script:Version is not available"
    }
    if ($script:CloudCurrent) { $script:Version = $currentVersion } else { $script:Version = $script:Cloud.version }
}

# The signed download address, if it is https on a host the manifest
# allows (or the site's own), with no user, password or port.
function CloudUrl([string]$Path) {
    try { $body = ReadJsonFile $Path } catch { return '' }
    if (-not ($body -is [Collections.Specialized.OrderedDictionary]) -or -not ($body['url'] -is [string])) { return '' }
    $url = [string]$body['url']
    $match = [regex]::Match($url, '^https://(?<host>[^/?#]+)([/?][!-~]*)?\z')
    if (-not $match.Success) { return '' }
    $hostName = $match.Groups['host'].Value
    if ($hostName -cnotmatch $FeedHost) { return '' }
    $siteHost = $Site.Substring('https://'.Length)
    if (-not ($script:Cloud.hosts -ccontains $hostName) -and $hostName -cne $siteHost) { return '' }
    $parsed = $null
    if (-not [Uri]::TryCreate($url, [UriKind]::Absolute, [ref]$parsed) -or $parsed.Scheme -cne 'https' -or
        $parsed.Host -cne $hostName -or -not $parsed.IsDefaultPort -or $parsed.UserInfo) { return '' }
    return $url
}

# Downloads the resolved asset into $Work and checks its size and sha256.
function CloudDownload([string]$Asset, [string]$Work) {
    if ($Asset -cne $script:Cloud.asset) { Die "the release feed's asset is $($script:Cloud.asset), not $Asset" }
    Note "downloading $Asset"
    $feed = "$Site/cloud/api/v1/desktop/feed/assets/$($script:Cloud.id)"
    $answer = Join-Path $Work 'download-url.json'
    $destination = Join-Path $Work $Asset
    $status = FeedRequest "$feed/download-url" $answer $FeedLimit -Method Post -WithCredential
    if ($status -eq 200) {
        $url = CloudUrl $answer
        if (-not $url) { Die "the release feed's download address was refused" }
        # A signed address carries its own authorisation: no credential goes to it.
        $status = FeedRequest $url $destination $script:Cloud.size
    } elseif ($status -eq 501) {
        # No object storage behind this site: the feed streams the asset itself.
        $status = FeedRequest $feed $destination $script:Cloud.size -WithCredential
    } else {
        CloudRefused 'the download address' $status
    }
    if ($status -eq -1) { Die "$Asset is larger than the manifest gives" }
    if ($status -ne 200) {
        $said = if ($status) { "HTTP $status" } else { 'no answer' }
        Die "could not download $Asset ($said)"
    }
    if ((Get-Item -LiteralPath $destination).Length -ne $script:Cloud.size) { Die "$Asset is not the size the manifest gives" }
    if ((Sha256Of $destination) -ne $script:Cloud.sha256) { Die "$Asset does not match the manifest's sha256" }
    Note 'size and sha256 verified'
}

switch -CaseSensitive ($ReleaseFeed) {
    'github' { }
    'cloud' { CloudPreflight }
    default { Die 'this copy of install.ps1 names an unknown release feed' }
}

function XmlEscape([string]$Text) {
    return $Text.Replace('&', '&amp;').Replace('<', '&lt;').Replace('>', '&gt;').Replace('"', '&quot;').Replace("'", '&apos;')
}

# The app's own definition (gtk/src/service/windows.rs): at logon, run the
# core with no time limit, on battery too, restarting after a failure, one
# instance at a time.
function TaskXml([string]$Exe, [string]$Arguments, [bool]$Enabled) {
    # Variable names are case-insensitive: these must not reuse the
    # parameters' names, or the typed parameters take the escaped values.
    $exeText = XmlEscape $Exe
    $userText = XmlEscape $TaskUser
    $directoryText = XmlEscape (Split-Path -Parent $Exe)
    $argumentsText = XmlEscape $Arguments
    $enabledText = if ($Enabled) { 'true' } else { 'false' }
    return @"
<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Author>Agent Relay</Author>
    <Description>Keeps your agents connected and uploads running while Agent Relay is closed, and starts at login.</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>$userText</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>$userText</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>$enabledText</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
    <RestartOnFailure>
      <Interval>PT1M</Interval>
      <Count>3</Count>
    </RestartOnFailure>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>"$exeText"</Command>
      <Arguments>$argumentsText</Arguments>
      <WorkingDirectory>$directoryText</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
"@
}

function Schtasks([string[]]$Arguments) {
    $ErrorActionPreference = 'Continue'
    $output = & schtasks.exe @Arguments 2>&1
    return @{ ok = ($LASTEXITCODE -eq 0); text = (($output | ForEach-Object { $_.ToString() }) -join "`n").Trim() }
}

# The task's definition: the program it runs and its arguments, and whether
# it is enabled; nothing when there is no such task.
function TaskDefinition {
    $query = Schtasks @('/Query', '/TN', $Task, '/XML')
    if (-not $query.ok) { return $null }
    $xml = [xml]$query.text
    $ns = New-Object Xml.XmlNamespaceManager $xml.NameTable
    $ns.AddNamespace('t', 'http://schemas.microsoft.com/windows/2004/02/mit/task')
    $command = $xml.SelectSingleNode('//t:Actions/t:Exec/t:Command', $ns)
    $arguments = $xml.SelectSingleNode('//t:Actions/t:Exec/t:Arguments', $ns)
    $enabled = $xml.SelectSingleNode('//t:Settings/t:Enabled', $ns)
    return @{
        command = if ($command) { $command.InnerText.Trim().Trim('"') } else { '' }
        arguments = if ($arguments) { $arguments.InnerText.Trim() } else { '' }
        enabled = (-not $enabled) -or ($enabled.InnerText.Trim() -ne 'false')
    }
}

# Whether the task is running now: the verbose listing's Last Result is
# 267009 (SCHED_S_TASK_RUNNING) while it runs. Read by column, not by the
# display language's header, as the app does.
function TaskRunning {
    $listed = Schtasks @('/Query', '/TN', $Task, '/FO', 'CSV', '/V', '/NH')
    if (-not $listed.ok) { return $false }
    $headers = 0..39 | ForEach-Object { "c$_" }
    $rows = @($listed.text -split "`n" | Where-Object { $_.Trim() } | ConvertFrom-Csv -Header $headers)
    return @($rows | Where-Object { $_.c6 -eq '267009' }).Count -gt 0
}

function RegisterTask([bool]$Enabled) {
    $exe = Join-Path $Current $Probe
    $arguments = ServeArguments $exe
    if ($DryRun) { WouldRun "register the logon task $Task running $exe $arguments"; return $true }
    $file = Join-Path ([IO.Path]::GetTempPath()) ("agent-relay-task-" + [IO.Path]::GetRandomFileName() + ".xml")
    # schtasks reads UTF-16 with its byte-order mark.
    [IO.File]::WriteAllText($file, (TaskXml $exe $arguments $Enabled), [Text.Encoding]::Unicode)
    try {
        $created = Schtasks @('/Create', '/TN', $Task, '/XML', $file, '/F')
        if (-not $created.ok) { Note "schtasks could not register ${Task}: $($created.text)"; return $false }
        return $true
    } finally {
        Remove-Item -LiteralPath $file -Force -ErrorAction SilentlyContinue
    }
}

# Writes the task only when it is missing, names another program or runs
# it in the form the core `current` points at does not take.
function EnsureTask {
    $definition = TaskDefinition
    $wanted = Join-Path $Current $Probe
    if ($definition -and $definition.command -ieq $wanted -and $definition.arguments -eq (ServeArguments $wanted)) { return $true }
    return RegisterTask $true
}

# Restarts the task (enables it first when it is off). False when Task
# Scheduler could not start it.
function RestartTask {
    if ($DryRun) { WouldRun "restart the logon task $Task"; return $true }
    $definition = TaskDefinition
    if ($definition -and -not $definition.enabled) {
        $changed = Schtasks @('/Change', '/TN', $Task, '/ENABLE')
        if (-not $changed.ok) { Note "schtasks could not enable ${Task}: $($changed.text)"; return $false }
    }
    # Ending a task that is not running is not a failure.
    Schtasks @('/End', '/TN', $Task) | Out-Null
    Start-Sleep -Seconds 1
    $ran = Schtasks @('/Run', '/TN', $Task)
    if (-not $ran.ok) { Note "schtasks could not start ${Task}: $($ran.text)"; return $false }
    return $true
}

# Ends the task, and with it the core it runs.
function StopTask {
    if ($DryRun) { WouldRun "end the logon task $Task"; return }
    Schtasks @('/End', '/TN', $Task) | Out-Null
    Start-Sleep -Seconds 1
}

# Moves a folder, waiting a little for a core that is ending to let go of
# the files in it. False when it could not.
function MoveFolder([string]$From, [string]$To) {
    for ($attempt = 1; $attempt -le 10; $attempt++) {
        try { Move-Item -LiteralPath $From -Destination $To; return $true } catch { Start-Sleep -Seconds 1 }
    }
    return $false
}

# Puts back the copy of $Version this install set aside, if there was one.
# The core must not be running from $destination.
function RestoreReplaced {
    $replaced = "$destination.replaced"
    if (-not (Test-Path -LiteralPath $replaced)) { return $true }
    for ($attempt = 1; $attempt -le 10 -and (Test-Path -LiteralPath $destination); $attempt++) {
        try { Remove-Item -LiteralPath $destination -Recurse -Force } catch { Start-Sleep -Seconds 1 }
    }
    return MoveFolder $replaced $destination
}

# One run at a time per prefix: a person and an agent starting the same
# install together would otherwise stage into the same folder and switch
# `current` and the task under each other. The lock is held until Finish.
# A dry run changes nothing, so it creates neither the folder nor the lock.
if (-not $DryRun) {
    New-Item -ItemType Directory -Path $Prefix -Force | Out-Null
    try {
        $script:InstallLock = [IO.File]::Open((Join-Path $Prefix 'install.lock'), [IO.FileMode]::OpenOrCreate, [IO.FileAccess]::ReadWrite, [IO.FileShare]::None)
    } catch [IO.IOException] {
        Die "another install is running for $Prefix; wait for it to finish"
    }
}

# --- 1. what is installed ---------------------------------------------------

$definition = TaskDefinition
if ($definition -and $definition.command -match '(?i)\\agent-relay\.exe$') {
    Note "the Agent Relay app owns the $Task task ($($definition.command))."
    Note 'update it from the app (Settings > Updates) or with a newer Setup; this script installs only the bare core.'
    Finish 3
}

$currentVersion = ''
$currentExe = Join-Path $Current $Probe
if (Test-Path -LiteralPath $currentExe -PathType Leaf) {
    $currentVersion = ProbeVersion $currentExe
}
if ($currentVersion) { Note "installed: $currentVersion" } else { Note 'no per-user install found' }
# The product of the installed version: its record, written on install, or
# Agent Relay's for an install made before there was one.
$currentProduct = ''
if ($currentVersion) {
    $record = Join-Path $Current 'product'
    if (Test-Path -LiteralPath $record -PathType Leaf) {
        $currentProduct = ((@(Get-Content -LiteralPath $record -TotalCount 1)) -join '').Trim()
    }
    if (-not $currentProduct) { $currentProduct = $DefaultAssetPrefix }
}

# The newest installed version older than the current one, so repeated
# rollbacks keep going back; with no current version, the newest installed.
function PreviousVersion {
    $numeric = { param($v) [Version](($v -split '[-+]')[0]) }
    $others = @(InstalledVersions | Where-Object { $_ -ne $currentVersion })
    if ($currentVersion) {
        $current = & $numeric $currentVersion
        $others = @($others | Where-Object { (& $numeric $_) -lt $current })
    }
    if ($others.Count -eq 0) { return '' }
    return $others[-1]
}

# --- rollback -----------------------------------------------------------------

if ($Rollback) {
    $target = PreviousVersion
    if (-not $target) { Die 'no earlier version to roll back to' }
    $was = $currentVersion
    $wasText = if ($was) { $was } else { '?' }
    Note "rolling back $wasText -> $target"
    SwitchTo $target
    EnsureTask | Out-Null
    RestartTask | Out-Null
    if (-not (Healthy $target)) {
        # The older core did not come back (it may not read settings a newer
        # one wrote): put the version that was running back, as an update
        # that fails does.
        if ($was -and (Test-Path -LiteralPath (Join-Path $Versions $was))) {
            Note "the core did not come back on $target; going back to $was"
            SwitchTo $was
            EnsureTask | Out-Null
            RestartTask | Out-Null
            if (-not (Healthy $was)) { Die "neither $target nor $was answered; $Log" }
            Die "rollback to $target failed and $was is running again; $Log"
        }
        Die "the core did not come back on $target; $Log"
    }
    Note "now running $target"
    Finish 0
}

# --- 2. which release -------------------------------------------------------

if ($ReleaseFeed -eq 'cloud') {
    $feedWork = Join-Path ([IO.Path]::GetTempPath()) ("agent-relay-feed-" + [IO.Path]::GetRandomFileName())
    New-Item -ItemType Directory -Path $feedWork -Force | Out-Null
    try { CloudResolve $feedWork } finally { Remove-Item -LiteralPath $feedWork -Recurse -Force -ErrorAction SilentlyContinue }
} elseif (-not $Version) {
    $latest = Join-Path ([IO.Path]::GetTempPath()) ("agent-relay-latest-" + [IO.Path]::GetRandomFileName())
    try {
        Fetch $ReleasesApi 'latest' $latest
        $tag = (Get-Content -LiteralPath $latest -Raw | ConvertFrom-Json).tag_name
    } catch {
        Die "could not read the latest release: $($_.Exception.Message)"
    } finally {
        Remove-Item -LiteralPath $latest -Force -ErrorAction SilentlyContinue
    }
    $Version = ([string]$tag) -replace '^v', ''
    if (-not (Test-ReleaseVersion $Version)) { Die 'the latest release has no usable version' }
}
Note "release: $Version ($Arch)"

if ($currentVersion -eq $Version -and $currentProduct -cne $AssetPrefix) {
    Note "the installed $Version is another product's; replacing it with $ProductName's"
}
if ($currentVersion -eq $Version -and $currentProduct -ceq $AssetPrefix) {
    if ($DryRun -or ((CoreVersion) -eq $Version -and (TaskRunning))) {
        Note 'already up to date'
        Finish 0
    }
    Note 'installed but not running from its task; restarting it'
    EnsureTask | Out-Null
    RestartTask | Out-Null
    if (-not (Healthy $Version)) { Die "the core did not start; $Log" }
    Finish 0
}

# --- 3. download, verify, unpack -------------------------------------------

$asset = "$AssetPrefix-$Version-Windows-$Arch-probe.zip"
$work = Join-Path ([IO.Path]::GetTempPath()) ("agent-relay-install-" + [IO.Path]::GetRandomFileName())
New-Item -ItemType Directory -Path $work -Force | Out-Null
try {
    if ($ReleaseFeed -eq 'cloud') {
        CloudDownload $asset $work
    } else {
        Note "downloading $asset"
        try { Fetch $DownloadBase "v$Version/$asset" (Join-Path $work $asset) } catch { Die "could not download ${asset}: $($_.Exception.Message)" }
        try { Fetch $DownloadBase "v$Version/$asset.sha256" (Join-Path $work "$asset.sha256") } catch { Die "could not download $asset.sha256: $($_.Exception.Message)" }
        $expected = ((Get-Content -LiteralPath (Join-Path $work "$asset.sha256") -TotalCount 1) -split '\s+')[0].ToLowerInvariant()
        if ($expected -notmatch '^[0-9a-f]{64}$') { Die 'the published checksum is unreadable' }
        if ((Sha256Of (Join-Path $work $asset)) -ne $expected) { Die "$asset does not match its published checksum" }
        Note 'checksum verified'
    }

    # Only the published layout is accepted out of the archive: the core and
    # the DLLs beside it in the helpers folder, and the license text next to
    # that folder (linux/install.sh accepts it as agent_relay/LICENSE), no
    # nested folders, nothing else.
    $unpacked = Join-Path $work 'unpacked'
    Expand-Archive -LiteralPath (Join-Path $work $asset) -DestinationPath $unpacked -Force
    $tree = Join-Path $unpacked 'agent_relay'
    $helpers = Join-Path $tree 'helpers'
    $entries = @(Get-ChildItem -LiteralPath $unpacked -Recurse -Force)
    $licenses = @($entries | Where-Object {
        -not $_.PSIsContainer -and $_.DirectoryName -eq $tree -and $_.Name -match '(?i)^(LICENSE|NOTICES?)(\.txt)?$'
    })
    $allowed = $entries | Where-Object {
        $_.FullName -eq $tree -or
        $_.FullName -eq $helpers -or
        (-not $_.PSIsContainer -and $_.DirectoryName -eq $helpers -and $_.Name -match '(?i)^[A-Za-z0-9._+-]+\.(exe|dll)$') -or
        $licenses -contains $_
    }
    if ($entries.Count -ne @($allowed).Count -or -not (Test-Path -LiteralPath (Join-Path $helpers $Probe) -PathType Leaf)) {
        Die 'the archive holds something other than the core'
    }
    $reported = ProbeVersion (Join-Path $helpers $Probe)
    if ($reported -ne $Version) {
        $said = if ($reported) { $reported } else { 'nothing' }
        Die "the downloaded core reports $said, not $Version"
    }

    $destination = Join-Path $Versions $Version
    if ($DryRun) {
        WouldRun "install $destination"
    } else {
        New-Item -ItemType Directory -Path $Versions -Force | Out-Null
        $partial = "$destination.partial"
        if (Test-Path -LiteralPath $partial) { Remove-Item -LiteralPath $partial -Recurse -Force }
        New-Item -ItemType Directory -Path $partial | Out-Null
        Copy-Item -Path (Join-Path $helpers '*') -Destination $partial -Force
        foreach ($license in $licenses) { Copy-Item -LiteralPath $license.FullName -Destination $partial -Force }
        [IO.File]::WriteAllText((Join-Path $partial 'product'), "$AssetPrefix`n", [Text.Encoding]::ASCII)
        # An existing copy of this version (another product's, or one left
        # over) is set aside until the new core answers, and comes back if
        # it does not. Windows does not move a folder whose core is running,
        # so the task is ended first when that is the installed version.
        $replaced = "$destination.replaced"
        if (Test-Path -LiteralPath $replaced) { Remove-Item -LiteralPath $replaced -Recurse -Force }
        if (Test-Path -LiteralPath $destination) {
            if ($currentVersion -eq $Version) { StopTask }
            if (-not (MoveFolder $destination $replaced)) {
                if ($currentVersion -eq $Version) { RestartTask | Out-Null }
                Die "could not set the installed $Version aside; $Log"
            }
        }
        Move-Item -LiteralPath $partial -Destination $destination
    }
} finally {
    Remove-Item -LiteralPath $work -Recurse -Force -ErrorAction SilentlyContinue
}

# --- 4. switch and restart ---------------------------------------------------

$was = $currentVersion
SwitchTo $Version
$registered = EnsureTask
# A restart Task Scheduler refuses rolls back like a core that never answers.
$restarted = $registered -and (RestartTask)
if (-not $restarted) { Note "Task Scheduler could not restart $Task" }

# --- 5. health, or roll back -------------------------------------------------

if (-not $restarted -or -not (Healthy $Version)) {
    if ($was -and $was -ne $Version -and (Test-Path -LiteralPath (Join-Path $Versions $was))) {
        Note "the new core did not answer; rolling back to $was"
        SwitchTo $was
        EnsureTask | Out-Null
        RestartTask | Out-Null
        # A version that failed is never a rollback target.
        if (-not $DryRun) { Remove-Item -LiteralPath (Join-Path $Versions $Version) -Recurse -Force -ErrorAction SilentlyContinue }
        RestoreReplaced | Out-Null
        if (-not (Healthy $was)) { Die "rollback to $was did not answer either; $Log" }
        Die "update to $Version failed and was rolled back to $was; $Log"
    }
    # The same version of another product: put its copy back and restart it.
    if ($was -and $was -eq $Version -and -not $DryRun -and (Test-Path -LiteralPath "$destination.replaced")) {
        Note "the new core did not answer; going back to the installed $was"
        StopTask
        if (-not (RestoreReplaced)) { Die "could not put the installed $was back; $Log" }
        RestartTask | Out-Null
        if (-not (Healthy $was)) { Die "going back to $was did not answer either; $Log" }
        Die "replacing the installed $was failed and it is running again; $Log"
    }
    Die "the core did not start; $Log"
}
Note "running $Version"
if (-not $DryRun) { Remove-Item -LiteralPath "$destination.replaced" -Recurse -Force -ErrorAction SilentlyContinue }

# Keep the newest KEEP versions (always the current one).
if (-not $DryRun) {
    $others = @(InstalledVersions | Where-Object { $_ -ne $Version })
    $remove = $others.Count - ($Keep - 1)
    if ($remove -gt 0) {
        $others | Select-Object -First $remove | ForEach-Object {
            Remove-Item -LiteralPath (Join-Path $Versions $_) -Recurse -Force -ErrorAction SilentlyContinue
        }
    }
}
Finish 0
