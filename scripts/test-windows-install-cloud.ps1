<#
.SYNOPSIS
test-windows-install-cloud.ps1 -- windows/install.ps1 on a site profile's
cloud release feed, dot-sourced by scripts/test-windows-install.ps1.

.DESCRIPTION
A copy of install.ps1 with the cloud header lines set, and its FeedExchange
(the one HTTP exchange) renamed out of the way, runs inside a wrapper whose
FeedExchange serves the feed's proposed routes from a local folder (the
manifest, the download address, the signed storage address and the streamed
fallback) and logs which credential each request carried. The fake core
from the parent script answers status.get with an account, prints feed
headers, and answers `feed verify` from a file. Checked: refused overrides,
no token on a first install, not_entitled, a key of another profile, a first
install without OpenSSL 3 refused unless the skipped check is allowed (and
then said), a download address on another host never contacted, an update
on the core's own headers through the streamed fallback, a tampered asset,
a refused manifest, not_newer, another product's core at the same version
replaced, and, when OpenSSL 3 is on the runner, a real signature checked by
OpenSSL and a tampered one refused.

Never run on its own: it uses the parent's $root, $prefix, $state, FakeZip,
Reset, Fail, Pass, RunningVersion and Installed.
#>

$cloud = Join-Path $root 'cloud'
$feedRoot = Join-Path $cloud 'feed'
$cloudInstall = Join-Path $cloud 'install.ps1'
$cloudRun = Join-Path $cloud 'run.ps1'
$cloudSite = 'https://relay.fixture.test'
$cloudProfile = 'fixtureprofile'
$cloudToken = 'install-fixture-' + [IO.Path]::GetRandomFileName().Replace('.', '')
$feedApi = "$cloudSite/cloud/api/v1/desktop/feed"
# A throwaway Ed25519 key's public half; its private half signed only the
# envelope below and was not kept.
$cloudKey = 'MXClEeEzMjoJu4DkLdhGP3GiUAIOCjlpn1lhbkGtBO4='
$signedEnvelope = @'
{"key_id": "fk_fixtureprofile_k1", "manifest": {"assets": [{"name": "FixtureRelay-2026.11.3-Windows-x64-probe.zip", "platform": "Windows", "arch": "x64", "kind": "probe", "id": "fixture_x64", "size": 4096, "sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}, {"name": "FixtureRelay-2026.11.3-Windows-arm64-probe.zip", "platform": "Windows", "arch": "arm64", "kind": "probe", "id": "fixture_arm64", "size": 4096, "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}], "download_hosts": ["storage.fixture.test"], "notes": "Fixes \u2014 \"quoted\", back\\slash,\ttab\nnew line, caf\u00e9 \ud83d\ude80 \u0001", "published_at": "2026-11-07T00:00:00Z", "version": "2026.11.3", "channel": "stable", "profile": "fixtureprofile", "schema": 1}, "signature": "SunsOop54RPpLMeSN5QVjsPX37cDp1-1vgLGYAKqYz3nZcvaSw1xRnrLg3cxjc85S4px0ibZLD0BBVHuOfJ7CA"}
'@

New-Item -ItemType Directory -Path $feedRoot -Force | Out-Null
$copy = [IO.File]::ReadAllText($install)
foreach ($line in @(
    @("`$ProductName = 'Agent Relay'", "`$ProductName = 'Fixture Relay'"),
    @("`$ReleasesRepository = 'AgentWorkforce/relay-desktop-releases'", "`$ReleasesRepository = ''"),
    @("`$AssetPrefix = 'AgentRelay'", "`$AssetPrefix = 'FixtureRelay'"),
    @("`$ReleaseFeed = 'github'", "`$ReleaseFeed = 'cloud'"),
    @("`$SiteUrl = ''", "`$SiteUrl = '$cloudSite'"),
    @("`$ProfileId = ''", "`$ProfileId = '$cloudProfile'"),
    @("`$FeedPublicKey = ''", "`$FeedPublicKey = '$cloudKey'"),
    @('function FeedExchange(', 'function FeedExchangeUnused('))) {
    if (-not $copy.Contains($line[0])) { Fail "install.ps1 has no line $($line[0])" }
    $copy = $copy.Replace($line[0], $line[1])
}
[IO.File]::WriteAllText($cloudInstall, $copy)

# The wrapper: a fake FeedExchange serving the feed folder (the copy's own
# is renamed, so the copy finds this one), and with -LibreSsl an `openssl`
# that is not OpenSSL 3, so a first install has no way to check the signature.
$wrapper = @'
param([switch]$DryRun, [string]$Version = '', [switch]$LibreSsl, [switch]$AllowUnverifiedFeed)
$feedRoot = '@FEED@'
$feedApi = '@API@'
function FeedExchange([string]$Uri, [string]$Method, [hashtable]$Headers, [string]$OutFile, [long]$Limit) {
    $auth = 'none'
    if ($Headers.ContainsKey('Authorization')) {
        $auth = 'other'
        if ($env:AGENT_RELAY_INSTALL_TOKEN -and $Headers['Authorization'] -ceq "Bearer $env:AGENT_RELAY_INSTALL_TOKEN") { $auth = 'install' }
        if ($Headers['Authorization'] -ceq 'Bearer core-fixture-credential') { $auth = 'core' }
    }
    $profileHeader = if ($Headers.ContainsKey('X-Agent-Relay-Profile')) { $Headers['X-Agent-Relay-Profile'] } else { '-' }
    $desktop = if ($Headers.ContainsKey('X-Agent-Relay-Desktop')) { $Headers['X-Agent-Relay-Desktop'] } else { '-' }
    Add-Content -LiteralPath (Join-Path $feedRoot 'requests.log') -Value "$Method $Uri auth=$auth profile=$profileHeader desktop=$desktop"
    $route = ''
    if ($Method -eq 'Get' -and $Uri -ceq "$feedApi/manifest?channel=stable") { $route = 'manifest' }
    elseif ($Method -eq 'Post' -and $Uri -cmatch "^$([regex]::Escape($feedApi))/assets/[A-Za-z0-9_-]+/download-url\z") { $route = 'download-url' }
    elseif ($Method -eq 'Get' -and $Uri -cmatch "^$([regex]::Escape($feedApi))/assets/[A-Za-z0-9_-]+\z") { $route = 'stream' }
    elseif ($Method -eq 'Get' -and $Uri -cmatch '^https://storage\.fixture\.test/') { $route = 'storage' }
    $status = 404
    $answer = [Text.Encoding]::UTF8.GetBytes('{"error":"not_found"}')
    if ($route) {
        $errorFile = Join-Path $feedRoot "$route.error"
        $file = Join-Path $feedRoot $route
        if (Test-Path -LiteralPath $errorFile) {
            $parts = ([IO.File]::ReadAllText($errorFile)).Split(' ', 2)
            $status = [int]$parts[0]
            $answer = [Text.Encoding]::UTF8.GetBytes($parts[1])
        } elseif (Test-Path -LiteralPath $file) {
            $status = 200
            $answer = [IO.File]::ReadAllBytes($file)
        }
    }
    if ($status -ne 200) { return @{ status = $status; body = [Text.Encoding]::UTF8.GetString($answer) } }
    if ($answer.Length -gt $Limit) { return @{ status = -1; body = '' } }
    [IO.File]::WriteAllBytes($OutFile, $answer)
    return @{ status = 200; body = '' }
}
if ($LibreSsl) { function openssl { 'LibreSSL 3.3.6' } }
& '@INSTALL@' -DryRun:$DryRun -Version $Version -AllowUnverifiedFeed:$AllowUnverifiedFeed
exit $LASTEXITCODE
'@
[IO.File]::WriteAllText($cloudRun, $wrapper.Replace('@FEED@', $feedRoot).Replace('@API@', $feedApi).Replace('@INSTALL@', $cloudInstall))

function CloudRun([string[]]$Arguments = @(), [string]$Token = '', [switch]$LibreSsl, [switch]$AllowUnverifiedFeed, [hashtable]$Extra = @{}) {
    $ErrorActionPreference = 'Continue'
    foreach ($name in 'AGENT_RELAY_RELEASES_API', 'AGENT_RELAY_DOWNLOAD_BASE', 'AGENT_RELAY_INSTALL_TOKEN') { Remove-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue }
    if ($Token) { $env:AGENT_RELAY_INSTALL_TOKEN = $Token }
    foreach ($name in $Extra.Keys) { Set-Item -LiteralPath "Env:$name" -Value $Extra[$name] }
    $env:AGENT_RELAY_PREFIX = $prefix
    $env:AGENT_RELAY_TASK = $task
    $env:AGENT_RELAY_SERVE_ARGS = "--socket `"$socket`" --app-data-dir `"$data`""
    $env:AGENT_RELAY_CORE_SOCKET = $socket
    $env:AGENT_RELAY_HEALTH_TIMEOUT = '30'
    if ($LibreSsl) { $Arguments += '-LibreSsl' }
    if ($AllowUnverifiedFeed) { $Arguments += '-AllowUnverifiedFeed' }
    $output = & powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $cloudRun @Arguments 2>&1 | ForEach-Object { $_.ToString() }
    $code = $LASTEXITCODE
    foreach ($name in @($Extra.Keys) + 'AGENT_RELAY_INSTALL_TOKEN') { Remove-Item -LiteralPath "Env:$name" -ErrorAction SilentlyContinue }
    $text = $output -join "`n"
    if ($text.Contains($cloudToken) -or $text.Contains('core-fixture-credential')) { Fail "a credential was printed:`n$text" }
    return @{ code = $code; text = $text }
}

function Requests {
    $log = Join-Path $feedRoot 'requests.log'
    if (-not (Test-Path -LiteralPath $log)) { return '' }
    $text = [IO.File]::ReadAllText($log)
    Remove-Item -LiteralPath $log -Force
    return $text
}

# The feed for one release: a manifest describing $Zip (or $Described, for
# a tampered asset) under a shape-valid signature that only the fake core's
# feed verify, or the skipped check, accepts.
function CloudRelease([string]$ReleaseVersion, [string]$Zip, [string]$Described = $Zip, [string]$KeyId = "fk_$($cloudProfile)_k1") {
    foreach ($file in Get-ChildItem -LiteralPath $feedRoot -File) { Remove-Item -LiteralPath $file.FullName -Force }
    $size = (Get-Item -LiteralPath $Described).Length
    $hash = (Get-FileHash -LiteralPath $Described -Algorithm SHA256).Hash.ToLowerInvariant()
    $name = "FixtureRelay-$ReleaseVersion-Windows-$arch-probe.zip"
    $manifest = "{`"schema`":1,`"profile`":`"$cloudProfile`",`"channel`":`"stable`",`"version`":`"$ReleaseVersion`",`"published_at`":`"2026-11-07T00:00:00Z`",`"notes`":`"`",`"download_hosts`":[`"storage.fixture.test`"],`"assets`":[{`"name`":`"$name`",`"platform`":`"Windows`",`"arch`":`"$arch`",`"kind`":`"probe`",`"id`":`"asset_$($arch)`",`"size`":$size,`"sha256`":`"$hash`"}]}"
    [IO.File]::WriteAllText((Join-Path $feedRoot 'manifest'), "{`"manifest`":$manifest,`"signature`":`"$('A' * 86)`",`"key_id`":`"$KeyId`"}")
    [IO.File]::WriteAllText((Join-Path $feedRoot 'download-url'), "{`"url`":`"https://storage.fixture.test/objects/asset_$($arch)?signature=fixture`",`"expires_at`":`"2026-11-07T01:00:00Z`"}")
    Copy-Item -LiteralPath $Zip -Destination (Join-Path $feedRoot 'storage') -Force
    Copy-Item -LiteralPath $Zip -Destination (Join-Path $feedRoot 'stream') -Force
}

function FeedError([string]$Route, [string]$Answer) { [IO.File]::WriteAllText((Join-Path $feedRoot "$Route.error"), $Answer) }
function VerifyAnswer([string]$Answer) {
    $path = Join-Path $state 'verify-answer'
    if ($Answer) { [IO.File]::WriteAllText($path, $Answer) } else { Remove-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue }
}
function FeedLog {
    $path = Join-Path $state 'feed.log'
    if (-not (Test-Path -LiteralPath $path)) { return '' }
    $text = [IO.File]::ReadAllText($path)
    Remove-Item -LiteralPath $path -Force
    return $text
}

Reset
$zipC1 = FakeZip '2026.11.1'
$zipC2 = FakeZip '2026.11.2'
CloudRelease '2026.11.1' $zipC1

$result = CloudRun -Token $cloudToken -Extra @{ AGENT_RELAY_DOWNLOAD_BASE = $releases }
if ($result.code -eq 0 -or $result.text -notmatch 'AGENT_RELAY_DOWNLOAD_BASE cannot be used') { Fail "a cloud copy took a download override ($($result.code)):`n$($result.text)" }
if (Requests) { Fail 'a refused override still reached the feed' }
Pass 'a cloud copy refuses the release overrides'

$result = CloudRun
if ($result.code -eq 0 -or $result.text -notmatch 'a first install needs AGENT_RELAY_INSTALL_TOKEN') { Fail "a first install ran without a token ($($result.code)):`n$($result.text)" }
if (Test-Path -LiteralPath (Join-Path $prefix 'versions')) { Fail 'a first install without a token wrote the prefix' }
Pass 'a first install without a token is refused'

FeedError 'manifest' '403 {"error":"not_entitled"}'
$result = CloudRun -Token $cloudToken
if ($result.code -eq 0 -or $result.text -notmatch 'not entitled to Fixture Relay \(not_entitled\)') { Fail "not_entitled was not reported ($($result.code)):`n$($result.text)" }
Pass 'a workspace without the entitlement is told so'

CloudRelease '2026.11.1' $zipC1 -KeyId 'fk_otherprofile_k1'
$result = CloudRun -Token $cloudToken -LibreSsl
if ($result.code -eq 0 -or $result.text -notmatch 'key_id_refused') { Fail "a key of another profile was accepted ($($result.code)):`n$($result.text)" }
Pass "a manifest signed with another profile's key is refused"

CloudRelease '2026.11.1' $zipC1
$result = CloudRun -Token $cloudToken -LibreSsl
if ($result.code -eq 0 -or $result.text -notmatch 'install OpenSSL 3 \(openssl\) and run this again') { Fail "a first install without OpenSSL 3 was not refused ($($result.code)):`n$($result.text)" }
if (Test-Path -LiteralPath (Join-Path $prefix 'versions')) { Fail 'a first install without OpenSSL 3 left an install behind' }
if ((Requests) -match 'download-url') { Fail 'a first install without OpenSSL 3 asked for the asset' }
Pass 'a first install without OpenSSL 3 is refused and asks for it'

# A download address on a host the manifest does not list is refused and
# never contacted (the opt-in switch lets the install get that far).
[IO.File]::WriteAllText((Join-Path $feedRoot 'download-url'), "{`"url`":`"https://elsewhere.fixture.test/objects/asset_$($arch)?signature=fixture`",`"expires_at`":`"2026-11-07T01:00:00Z`"}")
$result = CloudRun -Token $cloudToken -LibreSsl -AllowUnverifiedFeed
if ($result.code -eq 0 -or $result.text -notmatch "the release feed's download address was refused") { Fail "an off-host download address was accepted ($($result.code)):`n$($result.text)" }
$requests = Requests
if ($requests -notmatch 'POST \S+/assets/\S+/download-url auth=install') { Fail "the download address was not asked for:`n$requests" }
if ($requests -match 'elsewhere\.fixture\.test') { Fail "the off-host address was contacted:`n$requests" }
if (Test-Path -LiteralPath (Join-Path $prefix 'versions\2026.11.1')) { Fail 'an off-host download left an install behind' }
Pass 'a download address on another host is refused and never contacted'

CloudRelease '2026.11.1' $zipC1
$result = CloudRun -Token $cloudToken -LibreSsl -Extra @{ AGENT_RELAY_ALLOW_UNVERIFIED_FEED = '1' }
if ($result.code -ne 0 -or $result.text -notmatch 'running 2026\.11\.1') { Fail "first cloud install ($($result.code)):`n$($result.text)" }
if ($result.text -notmatch 'signature check was skipped' -or $result.text -notmatch 'allowed by -AllowUnverifiedFeed' -or $result.text -notmatch 'size and sha256 verified') { Fail "the skipped check was not said:`n$($result.text)" }
if (([IO.File]::ReadAllText((Join-Path $prefix 'versions\2026.11.1\product'))).Trim() -cne 'FixtureRelay') { Fail 'the product was not recorded' }
if ((RunningVersion) -ne '2026.11.1') { Fail 'the task is not running 2026.11.1' }
$requests = Requests
if ($requests -notmatch "GET $([regex]::Escape($feedApi))/manifest\?channel=stable auth=install profile=$cloudProfile desktop=-") { Fail "the manifest was not asked for with the token:`n$requests" }
if ($requests -notmatch 'POST \S+/assets/\S+/download-url auth=install') { Fail "the download address was not asked for with the token:`n$requests" }
if ($requests -notmatch 'GET https://storage\.fixture\.test/\S+ auth=none profile=- desktop=-' -or
    $requests -match 'storage\.fixture\.test/\S+ auth=(install|core|other)') { Fail "the signed address got a credential:`n$requests" }
Pass 'a first install allowed to skip the check says so, records its product and keeps the token off storage'

CloudRelease '2026.11.2' $zipC2 -Described $zipC1
$result = CloudRun
if ($result.code -eq 0 -or $result.text -notmatch "(larger than the manifest gives|not the size the manifest gives|does not match the manifest's sha256)") { Fail "a tampered asset was accepted ($($result.code)):`n$($result.text)" }
if (Test-Path -LiteralPath (Join-Path $prefix 'versions\2026.11.2')) { Fail 'a tampered asset left a version behind' }
if ((RunningVersion) -ne '2026.11.1') { Fail 'a tampered asset disturbed the running core' }
$null = Requests
$null = FeedLog
Pass 'a tampered asset is refused and nothing changes'

CloudRelease '2026.11.2' $zipC2
FeedError 'download-url' '501 {"error":"no_object_storage"}'
$result = CloudRun
if ($result.code -ne 0 -or $result.text -notmatch 'running 2026\.11\.2' -or $result.text -notmatch 'verified by the installed core') { Fail "cloud update ($($result.code)):`n$($result.text)" }
$requests = Requests
if ($requests -notmatch "manifest\?channel=stable auth=core profile=$cloudProfile desktop=1") { Fail "the update did not use the core's headers:`n$requests" }
if ($requests -notmatch "GET $([regex]::Escape($feedApi))/assets/asset_$arch auth=core") { Fail "the streamed fallback was not used:`n$requests" }
if ($requests -match 'storage\.fixture\.test') { Fail "storage was asked after a 501:`n$requests" }
$calls = FeedLog
if ($calls -notmatch "feed-headers --site $([regex]::Escape($cloudSite)) --workspace ws_fixture --account user_fixture --profile $cloudProfile") { Fail "feed-headers was not asked for this account:`n$calls" }
if ($calls -notmatch "feed verify --profile $cloudProfile --public-key \S+ --installed-version 2026\.11\.1 ") { Fail "feed verify was not asked with the installed version:`n$calls" }
Pass "an update uses the core's headers, its feed verify and the streamed fallback"

VerifyAnswer '{"ok":false,"code":"not_newer","message":"fixture"}'
$result = CloudRun
if ($result.code -ne 0 -or $result.text -notmatch 'already up to date') { Fail "not_newer was not up to date ($($result.code)):`n$($result.text)" }
VerifyAnswer '{"ok":false,"code":"signature_invalid","message":"fixture"}'
CloudRelease '2026.11.3' $zipC2
$result = CloudRun
if ($result.code -eq 0 -or $result.text -notmatch 'refused \(signature_invalid\)') { Fail "a manifest the core refused was used ($($result.code)):`n$($result.text)" }
if ((RunningVersion) -ne '2026.11.2') { Fail 'a refused manifest disturbed the running core' }
VerifyAnswer ''
$null = Requests
$null = FeedLog
Pass "the core's not_newer and refusals are followed"

# Another product's core at the same version (its record says AgentRelay) is
# replaced, not up to date: its version is not given to feed verify.
[IO.File]::WriteAllText((Join-Path $prefix 'versions\2026.11.2\product'), "AgentRelay`n")
CloudRelease '2026.11.2' $zipC2
$result = CloudRun
$calls = FeedLog
if ($calls -notmatch 'feed verify ' -or $calls -match '--installed-version') { Fail "another product's version was given to feed verify:`n$calls" }
if ($result.code -ne 0 -or $result.text -notmatch "the installed 2026\.11\.2 is another product's; replacing it" -or $result.text -notmatch 'running 2026\.11\.2') {
    Fail "another product's core at the same version was not replaced ($($result.code)):`n$($result.text)"
}
if ($result.text -match 'already up to date') { Fail "another product's core was called up to date:`n$($result.text)" }
if (([IO.File]::ReadAllText((Join-Path $prefix 'versions\2026.11.2\product'))).Trim() -cne 'FixtureRelay') { Fail 'the replacement did not record its product' }
if (Test-Path -LiteralPath (Join-Path $prefix 'versions\2026.11.2.replaced')) { Fail 'the replaced copy was left behind' }
if ((RunningVersion) -ne '2026.11.2') { Fail 'the replacement is not running' }
$null = Requests
Pass "another product's core at the same version is replaced, not up to date"

# A real signature, checked by OpenSSL 3 (the runner's, or Git's).
$openssl = Get-Command openssl -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
$path = $env:PATH
if (-not $openssl -and (Test-Path -LiteralPath 'C:\Program Files\Git\usr\bin\openssl.exe')) {
    $env:PATH = "C:\Program Files\Git\usr\bin;$env:PATH"
    $openssl = Get-Command openssl -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
}
$opensslVersion = if ($openssl) { (@(& $openssl.Source version 2>$null) | Select-Object -First 1) -join '' } else { '' }
try {
    if ($opensslVersion -match '^OpenSSL 3\.') {
        Reset
        foreach ($file in Get-ChildItem -LiteralPath $feedRoot -File) { Remove-Item -LiteralPath $file.FullName -Force }
        [IO.File]::WriteAllText((Join-Path $feedRoot 'manifest'), $signedEnvelope.Trim())
        [IO.File]::WriteAllText((Join-Path $feedRoot 'download-url'), '{"url":"https://storage.fixture.test/objects/fixture"}')
        Copy-Item -LiteralPath $zipC1 -Destination (Join-Path $feedRoot 'storage') -Force
        $result = CloudRun -Token $cloudToken
        if ($result.text -notmatch 'release manifest verified by OpenSSL' -or $result.code -eq 0 -or $result.text -notmatch "(larger than the manifest gives|not the size the manifest gives|does not match the manifest's sha256)") {
            Fail "OpenSSL did not verify the signed manifest, or its asset was taken ($($result.code)):`n$($result.text)"
        }
        [IO.File]::WriteAllText((Join-Path $feedRoot 'manifest'), $signedEnvelope.Trim().Replace('"version": "2026.11.3"', '"version": "2026.11.4"'))
        $result = CloudRun -Token $cloudToken
        if ($result.code -eq 0 -or $result.text -notmatch 'signature_invalid') { Fail "OpenSSL accepted a tampered manifest ($($result.code)):`n$($result.text)" }
        if (Test-Path -LiteralPath (Join-Path $prefix 'versions')) { Fail 'a refused signature left an install behind' }
        $null = Requests
        Pass 'OpenSSL 3 verifies a signed manifest and refuses a tampered one'
    } else {
        Write-Host 'skip - no OpenSSL 3 on this runner; the OpenSSL signature path is not exercised'
    }
} finally { $env:PATH = $path }
