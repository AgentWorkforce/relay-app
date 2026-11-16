# Verifying a site build's headless install on a Mac

One run, in order, on a Mac that has a login session and no Agent Relay app.
It checks that a site build's `install-macos.sh` installs the core, signs it
in, answers, updates as a no-op and uninstalls cleanly. Placeholders:
`<site-url>` is your site's address, `<version>` the release, `<arch>` `arm64`
or `x64`, `<token>` the install token. The installer is described in
[macos/README.md](../macos/README.md#headless-no-app); every line below that
starts `agent-relay install:` is the installer's own. Lines marked `...` are
omitted. The dry run and install print more `note` lines than are quoted
here; the quoted ones are the ones to look for.

## 1. Download the installer and check it

From your workspace's downloads page (the cloud feed), not from GitHub, save
`install-macos.sh`. The page shows its sha256; if it also lists
`install-macos.sh.sha256`, save that too.

```sh
shasum -a 256 install-macos.sh
shasum -a 256 -c install-macos.sh.sha256   # only if you saved the file
```

Expected: the digest equals the one on the page, and the second command
prints `install-macos.sh: OK`. A different digest means the file is not the
published one: delete it and download it again.

## 2. Dry run

```sh
read -rs AGENT_RELAY_INSTALL_TOKEN && export AGENT_RELAY_INSTALL_TOKEN
bash ./install-macos.sh --dry-run
```

The first install needs the token from the downloads page; `read -rs` takes
it from a paste so it does not land in the shell history (unset it after
step 4). Expected, among others, and nothing changed on disk:

```
agent-relay install: no per-user install found
agent-relay install: release: <version> (<arch>)
agent-relay install: downloading AgentRelay-<version>-macOS-<arch>-probe.tar.gz
agent-relay install: size and sha256 verified
agent-relay install: signature verified
agent-relay install: would write /Users/<user>/Library/LaunchAgents/com.agentrelay.desktop.core.headless.plist
agent-relay install: would install /Users/<user>/.local/lib/agent-relay/versions/<version>
agent-relay install: running <version>
```

(The asset prefix in the file name is the site build's own.)

## 3. Probe the site's Control

No sign-in needed:

```sh
curl -fsS <site-url>/api/v1/auth/login-config
curl -fsS <site-url>/healthz
```

`"enrollment_keys": true` in the first answer is a hint that the site serves
the enrollment-key exchange (relay-control 0.5.24 or later); `/healthz`
reports the Control version. Setup does not read this field: it falls back
to the device code only when the exchange itself answers 404 (step 4, exit 2),
and any other refusal fails setup (exit 1). Without the field, expect the
404 fallback.

## 4. Install with an enrollment key

The key file is a regular file, owned by you, mode 600, no symlink:

```sh
chmod 600 /private/path/to/key
bash ./install-macos.sh --enrollment-key-file /private/path/to/key
echo "exit $?"
unset AGENT_RELAY_INSTALL_TOKEN
```

The installer checks the installed core with `agent-relay-probe setup
--help` for `--enrollment-key-file`; it never prints the key. Success:

```
agent-relay install: running <version>
agent-relay install: finishing setup with the one-time enrollment key
...
agent-relay install: enrollment setup complete
agent-relay install: check it with this installer's --status, or: /Users/<user>/.local/lib/agent-relay/current/agent-relay-probe relay call status.get
agent-relay install: logs: /Users/<user>/Library/Logs/Agent Relay
```

and `exit 0`. Setup removes the key file after the site accepts it. If the
file is still there, the installer instead prints `setup complete; the key
file remains because it was unused, changed, or could not be removed; delete
it if Agent Relay accepted it`.

Exit 2 means the site has no enrollment-key route (the exchange answered
404) and setup printed a device code:

```
agent-relay install: setup still needs the action shown above; a key file is removed only after a successful exchange
```

The install is still good and the key file is kept. Approve the device code
in a browser on any computer, or fix the key and run
`~/.local/lib/agent-relay/current/agent-relay-probe setup` again. Any other
failure prints `error: the core is installed, but enrollment setup failed;
correct the reported problem and rerun setup` and exits 1.

## 5. Status

```sh
bash ./install-macos.sh --status
```

```
agent-relay install: installed: <version> (/Users/<user>/.local/lib/agent-relay/current/agent-relay-probe)
agent-relay install: product: <asset prefix>
agent-relay install: versions kept: <version>
agent-relay install: LaunchAgent: /Users/<user>/Library/LaunchAgents/com.agentrelay.desktop.core.headless.plist
agent-relay install: launchd: com.agentrelay.desktop.core.headless is loaded in gui/<uid>, pid <pid>
agent-relay install: logs: /Users/<user>/Library/Logs/Agent Relay
agent-relay install: data (never removed by this script): /Users/<user>/Library/Application Support/com.agentrelay.desktop
agent-relay install: core: answering, version <version>
```

Exit 0. `core: not answering` exits 1.

## 6. launchd

```sh
launchctl print gui/$(id -u)/com.agentrelay.desktop.core.headless
```

Look for `state = running`, a `pid`, and `program` (and the first
`arguments`) equal to `/Users/<user>/.local/lib/agent-relay/current/agent-relay-probe`
followed by `relay serve --headless`. The job names `current`, a link to
`~/.local/lib/agent-relay/versions/<version>`, so an update changes the link,
not the plist.

## 7. The core answers

```sh
~/.local/lib/agent-relay/current/agent-relay-probe call status.get --json
~/.local/lib/agent-relay/current/agent-relay-probe call agents.list --json
```

`status.get` returns the snapshot; its `daemon.version` is `<version>` and it
shows the signed-in account. `agents.list` returns `{"agents":[...]}`: empty
on a new install, and an agent you connect on this Mac is listed once it is
registered ([relay core](relay-core.md)). Methods:
[agent-relay-probe](agent-relay-probe.md#relay-core).

## 8. Update when already current

```sh
bash ./install-macos.sh --update
```

```
agent-relay install: installed: <version>
agent-relay install: release: <version> (<arch>)
agent-relay install: already up to date
```

Nothing is downloaded or restarted, and the exit is 0. (`--update` before
any install fails with `--update: nothing is installed in <prefix>; run
without --update to install`.)

## 9. Uninstall

```sh
bash ./install-macos.sh --uninstall
```

```
agent-relay install: stopping com.agentrelay.desktop.core.headless
agent-relay install: removed the headless core (<version>) and its LaunchAgent
agent-relay install: kept settings, sign-in and agent registrations in: /Users/<user>/Library/Application Support/com.agentrelay.desktop
agent-relay install: kept logs in: /Users/<user>/Library/Logs/Agent Relay
agent-relay install: delete those yourself if you do not mean to install Agent Relay again
```

Removed: the core, its versions and the LaunchAgent. Kept: settings, sign-in,
agent registrations and logs. `--status` afterwards prints `not installed`
and exits 1.

## If it fails

- **No login session.** `error: there is no login session (gui/<uid>) for
  <user>: log in at the Mac's screen once (or turn on automatic login), then
  run this again; SSH alone cannot start a LaunchAgent`.
- **App installed or its core loaded.** `error: <app or core> ... The <product>
  app runs this core itself; use the app, or quit and delete it (then log out
  and in) before installing the headless core`.
- **Enrollment refused.** The site has no exchange (404): exit 2 and the
  device code (step 4). A key that was wrong, expired or already used fails
  setup with exit 1; mint a new key and rerun setup.
- **Checksum mismatch.** Step 1: download the file again. During install,
  `error: <asset> does not match the manifest's sha256`, or `error: the release
  feed's manifest signature does not verify (signature_invalid)`: stop and
  tell the site's administrator; do not run the core.
- **Copy without a site.** `error: this copy of install-macos.sh has no site,
  profile or feed key; download it again from your workspace's downloads
  page`.
- **First install without a token.** `error: a first install needs
  AGENT_RELAY_INSTALL_TOKEN: copy the install token from your workspace's
  downloads page and run this again with it set`.
