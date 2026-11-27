# Sourced by test-macos-install.sh. All commands run with its disposable HOME
# and fake macOS tools. Only the PATH argument of the installed job is changed
# when the test simulates launchd firing it.

schedule_tick() {
  TEST_PATH="$PATH" TEST_APPS="$AGENT_RELAY_SYSTEM_APPLICATIONS" python3 -I - "$1" <<'PY'
import os, plistlib, sys
with open(sys.argv[1], "rb") as f:
  job = plistlib.load(f)
args = job["ProgramArguments"]
assert args[:2] == ["/usr/bin/env", "-i"]
assert args[4:6] == ["PATH=/usr/bin:/bin:/usr/sbin:/sbin", "/bin/bash"]
assert args[7] == "--scheduled-update" and len(args) == 9
assert job["StartInterval"] == 86400 and "RunAtLoad" not in job
assert "TOKEN" not in repr(job) and "ENROLLMENT" not in repr(job)
args[4] = "PATH=" + os.environ["TEST_PATH"]
args[5:5] = ["AGENT_RELAY_SYSTEM_APPLICATIONS=" + os.environ["TEST_APPS"],
           "FAKE_UNAME_M=" + os.environ.get("FAKE_UNAME_M", "arm64")]
os.execv(args[0], args)
PY
}

schedule_cases() (
  unset AGENT_RELAY_RELEASES_API AGENT_RELAY_DOWNLOAD_BASE
  cat > "$root/bin/lipo" <<LIPO
#!/usr/bin/env bash
[ -f "\$1" ] && [ "\$2" = -verify_arch ] && [ "\$3" = "\$(cat "$root/state/native-arch")" ]
LIPO
  chmod 0755 "$root/bin/lipo"

  local native job runner before other="$root/other-profile.sh"
  runner="$prefix/.scheduled-update.sh"
  sed "s/^PROFILE_ID=.*/PROFILE_ID='other'/" "$INSTALL" > "$other"

  for native in arm64 x86_64; do
    export FAKE_UNAME_M="$native"
    RELEASE_ARCH=arm64
    [ "$native" != x86_64 ] || RELEASE_ARCH=x64
    export RELEASE_ARCH
    printf '%s\n' "$native" > "$root/state/native-arch"
    release 2099.1.1
    release 2099.1.2
    release 2099.1.3 unsigned
    printf '%s\n' 2099.1.1 > "$root/state/latest"
    bash "$INSTALL" --schedule-enable > "$root/state/out" 2>&1 && fail "enabled without an installed release"
    bash "$INSTALL" > "$root/state/out"
    bash "$INSTALL" --schedule-status | grep -q 'schedule: disabled' || fail "schedule was on by default"
    [ ! -e "$runner" ] || fail "normal install copied a runner"
    bash "$INSTALL" --schedule-enable --dry-run | grep -q 'would register' || fail "enable dry run failed"
    [ ! -e "$runner" ] || fail "dry run copied a runner"
    piped "$INSTALL" | bash -s -- --schedule-enable > "$root/state/out" 2>&1 && fail "enabled from stdin"
    AGENT_RELAY_RELEASES_API=https://override.test bash "$INSTALL" --schedule-enable > "$root/state/out" 2>&1 && fail "enabled with a feed override"
    bash "$other" --schedule-enable > "$root/state/out" 2>&1 && fail "enabled with a different profile"
    mv "$prefix/current/release-receipt" "$root/receipt"
    bash "$INSTALL" --schedule-enable > "$root/state/out" 2>&1 && fail "enabled without a receipt"
    mv "$root/receipt" "$prefix/current/release-receipt"
    touch "$root/state/refuse-signature"
    bash "$INSTALL" --schedule-enable > "$root/state/out" 2>&1 && fail "enabled with an untrusted signature"
    grep -q 'release signature policy' "$root/state/out" || fail "installed signature guard was not reached"
    rm "$root/state/refuse-signature"
    printf '%s\n' different > "$root/state/native-arch"
    bash "$INSTALL" --schedule-enable > "$root/state/out" 2>&1 && fail "enabled with an incompatible architecture"
    grep -q 'native .* architecture' "$root/state/out" || fail "architecture guard was not reached"
    printf '%s\n' "$native" > "$root/state/native-arch"

    AGENT_RELAY_INSTALL_TOKEN=discard-me AGENT_RELAY_ENROLLMENT_KEY_FILE=discard-me bash "$INSTALL" --schedule-enable
    bash "$INSTALL" --schedule-status | grep -q 'schedule: enabled' || fail "enabled schedule not reported"
    (shopt -s nullglob; set -- "$HOME/Library/LaunchAgents/$label".update.*.plist; [ "$#" = 1 ]) || fail "expected one update job"
    job="$(find "$HOME/Library/LaunchAgents" -name "$label.update.*.plist")"
    before="$(starts)"
    bash "$INSTALL" --schedule-enable
    [ "$(starts)" = "$before" ] || fail "repeated enable reloaded the job"

    : > "$root/state/curl.log"
    cp "$prefix/current/release-receipt" "$root/receipt"
    printf 'different binding\n' > "$prefix/current/release-receipt"
    schedule_tick "$job" > "$root/state/out" 2>&1 && fail "scheduled run accepted a replacement profile"
    [ ! -s "$root/state/curl.log" ] || fail "refused scheduled run reached the feed"
    cp "$root/receipt" "$prefix/current/release-receipt"

    printf '%s\n' 2099.1.3 > "$root/state/latest"
    schedule_tick "$job" > "$root/state/out" 2>&1 && fail "scheduled run installed an unsigned update"
    grep -q 'downloaded core is not signed' "$root/state/out" || fail "scheduled run missed the candidate signature check"
    [ "$(current)" = 2099.1.1 ] && [ ! -e "$root/state/ran-unsigned" ] || fail "unsigned update changed the core"
    printf '%s\n' 2099.1.2 > "$root/state/latest"
    schedule_tick "$job" > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "scheduled verified update failed"; }
    [ "$(current)" = 2099.1.2 ] || fail "scheduled update did not switch the core"
    printf '%s\n' 2099.1.1 > "$root/state/latest"
    schedule_tick "$job" > "$root/state/out" 2>&1 || fail "scheduled downgrade check failed"
    [ "$(current)" = 2099.1.2 ] || fail "scheduler downgraded the core"
    cp "$plist" "$root/core-plist"
    printf '\n<!-- another registration -->\n' >> "$plist"
    schedule_tick "$job" > "$root/state/out" 2>&1 && fail "scheduled run rewrote a different core registration"
    grep -q 'LaunchAgent does not match' "$root/state/out" || fail "core registration guard was not reached"
    cp "$root/core-plist" "$plist"
    touch "$root/state/bootout-fails"
    bash "$INSTALL" --uninstall > "$root/state/out" 2>&1 && fail "uninstall proceeded with an update job still loaded"
    [ -f "$job" ] && [ -f "$runner" ] && [ "$(current)" = 2099.1.2 ] || fail "failed bootout removed install files"
    rm "$root/state/bootout-fails"
    bash "$INSTALL" --schedule-disable --dry-run > "$root/state/out"
    [ -f "$job" ] && [ -f "$runner" ] || fail "disable dry run removed files"
    bash "$INSTALL" --schedule-disable
    bash "$INSTALL" --schedule-disable
    [ ! -e "$job" ] && [ ! -e "$runner" ] && [ "$(cat "$root/state/running")" = 2099.1.2 ] || fail "disable removed the core or left the schedule"
    bash "$INSTALL" --schedule-enable
    bash "$INSTALL" --uninstall > "$root/state/out"
    bash "$INSTALL" --uninstall > "$root/state/out"
    [ ! -e "$job" ] && [ ! -e "$prefix" ] && [ ! -e "$root/state/loaded-$(basename "$job" .plist)" ] || fail "uninstall left scheduler state"
    pass "scheduler defaults off, verifies profile, signature and architecture, updates and cleans up on $native"
  done
)
rm -f "$root/state/ran-unsigned"
schedule_cases
