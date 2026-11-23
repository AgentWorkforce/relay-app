# Moving a headless Mac from a self-built core to the signed site build

For a self-hosted operator whose Mac runs a core built from source, and who
now installs your site's signed build with its `install-macos.sh`. The
installer is described in [macos/README.md](../macos/README.md#headless-no-app);
[headless-mac-site-verification.md](headless-mac-site-verification.md) shows
what each step prints.

## A. Before touching the Mac

1. Check that your site's feed offers the macOS headless files. The
   downloads page (`{site}/cloud/dashboard/downloads`) lists `install-macos.sh`
   and `<asset prefix>-<version>-macOS-<arch>-probe.tar.gz` when they were
   published. A release published under a manifest limit too small for them
   leaves those three files out and says so in its run
   ([The signer's manifest limit](workspace-layout.md#the-signers-manifest-limit)).
2. Download `install-macos.sh` from that page, and keep the page open for its
   sha256.
3. Get an install token from the same page when you are ready to install. A
   first install needs it; a token that has expired or was revoked is refused.
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
3. Check the installer against the page's sha256:

   ```sh
   shasum -a 256 install-macos.sh
   ```

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
   key):

   ```sh
   read -rs AGENT_RELAY_INSTALL_TOKEN && export AGENT_RELAY_INSTALL_TOKEN  # paste the token, not in shell history
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
