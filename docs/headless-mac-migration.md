# Moving a headless Mac from a self-built core to the signed site build

For a self-hosted operator whose Mac runs a core built from source, and who
now installs your site's signed build with its `install-macos.sh`. The
installer is described in [macos/README.md](../macos/README.md#headless-no-app);
[headless-mac-site-verification.md](headless-mac-site-verification.md) shows
what each step prints.

## A. Before touching the Mac

1. Check that the release you install carries the macOS headless files:
   `install-macos.sh` and
   `<asset prefix>-<version>-macOS-<arch>-probe.tar.gz` for each CPU. A
   release published under a manifest limit too small for them leaves those
   three files out and says so in its run
   ([The signer's manifest limit](workspace-layout.md#the-signers-manifest-limit)).
   The downloads page (`{site}/cloud/dashboard/downloads`) lists
   `install-macos.sh` with the command-line install scripts when it was
   published. It does not list the archives: they are in the release's
   signed manifest, which the dry run in B7 reads.
2. Download `install-macos.sh` from that page. The page shows no checksum for
   a script; the manifest records one, and B3 compares against it.
3. Get an install token when you are ready to install: it lasts 1 hour and
   reads your profile's feed only. A first install needs it; a token that has
   expired or was revoked is refused. The site mints it for a signed-in
   browser session only. Use the downloads page where it offers one;
   otherwise send, from that signed-in session, a `POST` to
   `{site}/cloud/api/v1/desktop/feed/install-tokens` with the header
   `X-Agent-Relay-Profile: <profile id>`. The answer shows the token once.
4. Decide how the Mac signs in: an administrator's enrollment key file, or
   the device code the installer prints.
5. Find out how the self-built core runs today: under the installer's
   LaunchAgent (`~/Library/LaunchAgents/com.agentrelay.desktop.core.headless.plist`,
   core in `~/.local/lib/agent-relay`), or some other way you set up.

## B. On the Mac

1. Log in at the Mac's screen as the user who will run the core. SSH alone
   cannot start a LaunchAgent. For an unattended Mac, turn on automatic login
   (FileVault must be off).
2. Install what a first install needs: `curl`, `python3` (the Command Line
   Tools, `xcode-select --install`) and Homebrew's `openssl@3` first on
   `PATH`. The Mac's own `openssl` is LibreSSL, which the installer refuses.
3. Export the install token, then check the installer against the sha256
   the feed records for it. The site and the profile id are in the script's
   own `SITE_URL=` and `PROFILE_ID=` lines:

   ```sh
   read -rs AGENT_RELAY_INSTALL_TOKEN && export AGENT_RELAY_INSTALL_TOKEN  # paste the token, not in shell history
   grep -E "^(SITE_URL|PROFILE_ID)=" install-macos.sh
   printf 'Authorization: Bearer %s\n' "$AGENT_RELAY_INSTALL_TOKEN" \
     | curl -fsS -H @- -H "X-Agent-Relay-Profile: <profile id>" \
         "<site>/cloud/api/v1/desktop/feed/manifest?channel=stable" \
     | python3 -c 'import json, sys; print(next(a["sha256"] for a in json.load(sys.stdin)["manifest"]["assets"] if a["name"] == "install-macos.sh"))'
   shasum -a 256 install-macos.sh
   ```

   The last two lines must show the same value. This compares your download
   with the feed's record over the same connection to your site; the
   manifest's signature is checked by the installer itself, in B7.

4. See what is installed now:

   ```sh
   bash ./install-macos.sh --status
   ```

5. Stop and remove the self-built core:
   - If `--status` shows it installed, run
     `bash ./install-macos.sh --uninstall`. It stops the core and removes
     its versions and LaunchAgent, so none of them is kept as a rollback
     target; settings, sign-in, agents and logs stay.
   - If it runs some other way, stop it and remove its LaunchAgent or service
     yourself. The installer does not look for it, and both cores serve the
     same socket: the second to start exits.
   - Delete a self-built `~/.local/bin/agent-relay-probe`. The site build
     never replaces a copy that is not signed like itself.
6. Run `--status` again; it should say `not installed`.
7. Dry run, then install (with `--enrollment-key-file` only if you have a
   key), in the shell that holds the token from B3. The dry run stops with
   `the release has no <archive name>` when the release left this Mac's
   archive out:

   ```sh
   bash ./install-macos.sh --dry-run
   bash ./install-macos.sh --enrollment-key-file /private/path/to/key
   unset AGENT_RELAY_INSTALL_TOKEN
   ```

   The installer checks the manifest's signature, the archive's size and
   sha256, and the core's Developer ID signature before it runs the core.
8. Without a key, finish sign-in with the `agent-relay-probe setup` command
   the installer prints.
9. Run `bash ./install-macos.sh --status` and check that the core answers
   with the site build's version
   ([steps 5 to 7](headless-mac-site-verification.md#5-status)).

## C. Afterwards

1. Update with `bash ./install-macos.sh --update`. A signed-in core needs no
   install token; otherwise export a fresh one first. The installer does not
   update on a schedule.
2. If the new core does not answer after an update, the installer switches
   back to the previous version by itself. Three versions are kept.
3. `bash ./install-macos.sh --rollback` switches to the newest installed
   version older than the current one.
4. `bash ./install-macos.sh --uninstall` removes the site build and keeps its
   data and logs.

## If it fails

See [If it fails](headless-mac-site-verification.md#if-it-fails).
