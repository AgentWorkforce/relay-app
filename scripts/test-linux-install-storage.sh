#!/usr/bin/env bash
# shellcheck shell=bash disable=SC2154
# Sourced by test-linux-install.sh; all paths belong to its disposable HOME.
# It deletes under HOME and writes the unit under XDG_CONFIG_HOME, so it
# refuses to run on its own or against directories the harness did not make.
# shellcheck source=scripts/install-test-home.sh
. "$(dirname "${BASH_SOURCE[0]}")/install-test-home.sh"
install_test_fragment
storage="$HOME/.agentworkforce/probe/store"
make_storage() { # version [source phase]; real SQLite, no production data
  python3 - "$storage" "$1" "${2:-done}" <<'PY'
import json, pathlib, sqlite3, sys
root = pathlib.Path(sys.argv[1]); root.mkdir(parents=True, exist_ok=True)
for name, phase in [('history.db', sys.argv[3]), ('delivery-queue.db', 'ready')]:
    c = sqlite3.connect(root / name)
    c.execute('CREATE TABLE IF NOT EXISTS _relay_delivery_sidecar(singleton PRIMARY KEY, phase, manifest)')
    c.execute('INSERT OR REPLACE INTO _relay_delivery_sidecar VALUES(1,?,?)', (phase, json.dumps({'version':int(sys.argv[2])})))
    c.execute('CREATE TABLE IF NOT EXISTS preserved(id PRIMARY KEY, bytes BLOB)')
    c.execute("INSERT OR IGNORE INTO preserved VALUES('identity',X'00017bff')")
    c.commit(); c.close()
PY
}
storage_rows() {
  python3 - "$storage" <<'PY'
import pathlib, sqlite3, sys
for p in sorted(pathlib.Path(sys.argv[1]).glob('*.db')):
    c = sqlite3.connect(p.as_uri()+'?mode=ro', uri=True)
    present = c.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name='preserved'").fetchone()
    print(p.name, c.execute('SELECT id,hex(bytes) FROM preserved').fetchall() if present else []); c.close()
PY
}
storage_refused() { # arguments passed to the real installer
  local before_current before_restarts
  before_current="$(current)"; before_restarts="$(restarts)"
  if bash "$INSTALL" "$@" > "$root/state/out" 2>&1; then
    fail "incompatible storage activation accepted: $*"
  fi
  grep -Fq "${STORAGE_REFUSAL_PATTERN:-cannot read the upgraded local history}" "$root/state/out" \
    || { cat "$root/state/out"; fail "missing actionable storage refusal"; }
  [ "$(current)" = "$before_current" ] || fail "storage refusal changed current"
  [ "$(restarts)" = "$before_restarts" ] || fail "storage refusal restarted the core"
}

make_legacy_storage() {
  require_disposable_home
  rm -rf "$storage"
  python3 - "$storage" <<'PY'
import pathlib, sqlite3, sys
root = pathlib.Path(sys.argv[1]); root.mkdir(parents=True)
c = sqlite3.connect(root / 'history.db')
c.execute('CREATE TABLE delivery_jobs(id PRIMARY KEY, bytes BLOB)')
c.execute('CREATE TABLE preserved(id PRIMARY KEY, bytes BLOB)')
c.execute("INSERT INTO preserved VALUES('identity',X'00017bff')")
c.execute("INSERT INTO delivery_jobs VALUES('queued',X'010203ff')")
c.commit(); c.close()
PY
}

# Genuine pre-sidecar storage stays usable by an old release without the
# capability command. Neither activation nor rollback rewrites its queue.
make_legacy_storage
saved_rows="$(storage_rows)"
legacy_sum="$(sha256sum "$storage/history.db")"
release 2.2.0
bash "$INSTALL" --version 2.2.0 > "$root/state/out" 2>&1 || fail "legacy version activation refused"
bash "$INSTALL" --rollback > "$root/state/out" 2>&1 || fail "legacy rollback refused"
if [ "$(storage_rows)" != "$saved_rows" ] || [ "$(sha256sum "$storage/history.db")" != "$legacy_sum" ]; then
  fail "legacy activation changed rows"
fi
pass "marker-less legacy history survives version selection and rollback without a capability command"

python3 - "$storage/history.db" <<'PY'
import sqlite3, sys
c = sqlite3.connect(sys.argv[1]); c.execute('DROP TABLE delivery_jobs')
c.execute('CREATE VIEW delivery_jobs AS SELECT 1 AS moved'); c.commit(); c.close()
PY
STORAGE_REFUSAL_PATTERN='local history has an unverified upload migration' storage_refused --version 2.2.0
pass "a marker-less history with the retired delivery_jobs view refuses activation"
require_disposable_home
rm -rf "$storage"

QUEUE_FORMATS=0,1 release 3.0.0
bash "$INSTALL" --version 3.0.0 > "$root/state/out" 2>&1 || fail "compatible release did not install"
make_storage 1
saved_rows="$(storage_rows)"
storage_refused --rollback
pass "a healthy legacy core cannot be selected by --rollback after migration"
storage_refused --version 2.1.0
pass "an explicit downgrade refuses before replacing or restarting the core"
[ "$(storage_rows)" = "$saved_rows" ] || fail "a refusal changed queued bytes or identities"
pass "refused downgrades retain exact evidence and immutable queue bytes"

QUEUE_FORMATS=0,1 release 3.1.0
bash "$INSTALL" --version 3.1.0 > "$root/state/out" 2>&1 || fail "compatible update refused"
bash "$INSTALL" --rollback > "$root/state/out" 2>&1 || fail "compatible rollback refused"
[ "$(current)" = 3.0.0 ] || fail "compatible rollback selected the wrong version"
pass "rollback to a binary that supports the migrated format still works"

# A cleanly closed WAL database can have no -wal/-shm files. In a directory
# where SQLite cannot create -shm, mode=ro fails and immutable=1 is the only
# read-only form that works. It is permitted only with the service stopped,
# both sidecars absent and collector.lock held by the installer.
if [ "$(id -u)" = 0 ]; then
  pass "clean WAL immutable-read guards are skipped as root (root bypasses directory mode bits)"
else
python3 - "$storage" <<'PY'
import pathlib, sqlite3, sys
for path in pathlib.Path(sys.argv[1]).glob('*.db'):
    c = sqlite3.connect(path)
    c.execute('PRAGMA journal_mode=WAL')
    c.commit(); c.close()
    pathlib.Path(str(path) + '-wal').unlink(missing_ok=True)
    pathlib.Path(str(path) + '-shm').unlink(missing_ok=True)
PY
touch "$storage/collector.lock" "$storage/delivery-queue.migration-lock"
chmod 0555 "$storage"
before_current="$(current)"; before_restarts="$(restarts)"
if bash "$INSTALL" --version 3.1.0 > "$root/state/out" 2>&1; then
  fail "a running service allowed an immutable SQLite check"
fi
grep -Fq 'could not verify local history compatibility' "$root/state/out" \
  || { cat "$root/state/out"; fail "the running-service guard did not report a storage refusal"; }
if [ "$(current)" != "$before_current" ] || [ "$(restarts)" != "$before_restarts" ]; then
  fail "a refused immutable check changed or restarted the core"
fi

# The service is stopped at the shell check, then appears after the collector
# lock is taken. The in-process recheck must still refuse immutable=1.
rm "$root/state/running"
touch "$root/state/activate-on-second-check"
: > "$root/state/active-checks"
if bash "$INSTALL" --version 3.1.0 > "$root/state/out" 2>&1; then
  fail "a service that started after the collector lock allowed an immutable SQLite check"
fi
grep -Fq 'could not verify local history compatibility' "$root/state/out" \
  || { cat "$root/state/out"; fail "the post-lock service guard did not report a storage refusal"; }
[ "$(cat "$root/state/active-checks")" = 2 ] \
  || fail "the service was checked $(cat "$root/state/active-checks") times, not before and after the lock"
rm -f "$root/state/activate-on-second-check" "$root/state/active-checks" "$root/state/running"

before_sum="$(sha256sum "$storage/history.db" "$storage/delivery-queue.db")"
bash "$INSTALL" --version 3.1.0 > "$root/state/out" 2>&1 \
  || { cat "$root/state/out"; fail "a stopped service could not verify clean WAL databases"; }
[ "$(sha256sum "$storage/history.db" "$storage/delivery-queue.db")" = "$before_sum" ] \
  || fail "the immutable compatibility check changed a database"
rm "$root/state/running"
exec {collector_fd}>>"$storage/collector.lock"
flock -n "$collector_fd"
if bash "$INSTALL" --version 3.1.0 > "$root/state/out" 2>&1; then
  fail "a held collector lock allowed an immutable SQLite check"
fi
grep -Fq 'could not verify local history compatibility' "$root/state/out" \
  || { cat "$root/state/out"; fail "the collector-lock guard did not report a storage refusal"; }
flock -u "$collector_fd"; exec {collector_fd}>&-
for suffix in wal shm; do
  chmod 0755 "$storage"; touch "$storage/history.db-$suffix"; chmod 0555 "$storage"
  if bash "$INSTALL" --version 3.1.0 > "$root/state/out" 2>&1; then
    fail "a present -$suffix sidecar allowed an immutable SQLite check"
  fi
  grep -Fq 'could not verify local history compatibility' "$root/state/out" \
    || { cat "$root/state/out"; fail "the -$suffix guard did not report a storage refusal"; }
  chmod 0755 "$storage"; rm "$storage/history.db-$suffix"; chmod 0555 "$storage"
done
chmod 0700 "$storage"
bash "$INSTALL" --version 3.1.0 > "$root/state/out" 2>&1 \
  || { cat "$root/state/out"; fail "the service did not restart after the immutable guards"; }
bash "$INSTALL" --rollback > "$root/state/out" 2>&1 \
  || { cat "$root/state/out"; fail "the clean-WAL fixture did not restore the earlier version"; }
[ "$(current)" = 3.0.0 ] || fail "the clean-WAL fixture left $(current), not 3.0.0"
pass "clean WAL databases use immutable reads only with no service, sidecars or collector"
fi

QUEUE_FORMATS=0,1 release 3.2.0 broken
if bash "$INSTALL" --version 3.2.0 > "$root/state/out" 2>&1; then fail "broken core accepted"; fi
[ "$(current)" = 3.0.0 ] || fail "compatible automatic rollback did not run"
pass "a failed start can roll back to a storage-compatible binary"

# A crash before the copy commit may leave a zero-byte destination. It has
# no user objects to protect; the frozen source still requires format 1.
make_storage 1 frozen
require_disposable_home
rm "$storage/delivery-queue.db"
: > "$storage/delivery-queue.db"
saved_rows="$(storage_rows)"
ASSERT_STORAGE_UNLOCKED_DURING_DOWNLOAD=1 bash "$INSTALL" --version 3.1.0 > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "empty interrupted destination blocked compatible recovery"; }
if [ "$(storage_rows)" != "$saved_rows" ] || [ -s "$storage/delivery-queue.db" ]; then
  fail "empty destination recovery altered storage"
fi
bash "$INSTALL" --rollback > "$root/state/out" 2>&1 || fail "compatible recovery rollback refused"
pass "an empty marker-less destination permits recovery without holding locks during downloads"

# Refusal branches must leave the active binary, restart count and records
# alone, independently of a core's status.get response.
make_storage 1
python3 - "$storage/delivery-queue.db" <<'PY'
import sqlite3, sys
c = sqlite3.connect(sys.argv[1]); c.execute('DROP TABLE _relay_delivery_sidecar'); c.commit(); c.close()
PY
saved_rows="$(storage_rows)"
STORAGE_REFUSAL_PATTERN='local upload queue has no migration metadata' storage_refused --version 3.1.0
[ "$(storage_rows)" = "$saved_rows" ] || fail "unverified queue refusal changed rows"
pass "a marker-less destination with user objects is refused"
python3 - "$storage/delivery-queue.db" <<'PY'
import sqlite3, sys
c = sqlite3.connect(sys.argv[1]); c.execute('DROP TABLE preserved')
c.execute('CREATE TABLE sqliteXcustomer(id)'); c.execute('INSERT INTO sqliteXcustomer VALUES(1)')
c.commit(); c.close()
PY
STORAGE_REFUSAL_PATTERN='local upload queue has no migration metadata' storage_refused --version 3.1.0
pass "a user table resembling a SQLite internal name is not ignored"
mv "$storage/delivery-queue.db" "$storage/queue.saved"
ln -s queue.saved "$storage/delivery-queue.db"
STORAGE_REFUSAL_PATTERN='local history format cannot be verified' storage_refused --version 3.1.0
pass "a symlinked destination is refused"
require_disposable_home
rm "$storage/delivery-queue.db"; mv "$storage/queue.saved" "$storage/delivery-queue.db"
make_storage 1
python3 - "$storage/history.db" <<'PY'
import sqlite3, sys
c = sqlite3.connect(sys.argv[1]); c.execute("UPDATE _relay_delivery_sidecar SET manifest='[]'"); c.commit(); c.close()
PY
STORAGE_REFUSAL_PATTERN='local history migration metadata is invalid' storage_refused --version 3.1.0
pass "a non-object migration manifest is refused"
make_storage 1
for reply in '{"protocol":2,"upload_queue_formats":[0,1]}' 'not-json' '{"protocol":1,"upload_queue_formats":[0,"1"]}' '{"protocol":1,"upload_queue_formats":[0,true]}'; do
  CAPABILITY_RAW="$reply" release 3.5.0
  STORAGE_REFUSAL_PATTERN='the target core returned invalid local history capabilities' storage_refused --version 3.5.0
done
CAPABILITY_RAW="$(python3 -c 'print("x" * 4097)')" release 3.5.0
STORAGE_REFUSAL_PATTERN='the target core returned invalid local history capabilities' storage_refused --version 3.5.0
pass "wrong protocols, invalid JSON, oversized replies and non-integer capabilities are refused"

make_storage 2
storage_refused --version 3.1.0
pass "an unknown future queue format is not guessed compatible"
make_storage 1 frozen
require_disposable_home
rm "$storage/delivery-queue.db"
storage_refused --version 2.1.0
pass "a frozen source is protected even before the sidecar file exists"
make_storage 1

exec {migration_fd}>"$storage/delivery-queue.migration-lock"
flock -n "$migration_fd"
before="$(restarts)"
if bash "$INSTALL" --version 3.1.0 > "$root/state/out" 2>&1; then fail "active migration lock ignored"; fi
grep -q 'local session history is upgrading' "$root/state/out" || fail "migration contention hidden"
[ "$(restarts)" = "$before" ] || fail "active migration restarted core"
flock -u "$migration_fd"; exec {migration_fd}>&-
pass "an active migration prevents concurrent installer activation"

# An old same-version product may answer healthy but lacks queue support.
cp "$HOME/.local/lib/agent-relay/current/agent-relay-probe" "$root/state/compatible-core"
sed 's/\[0,1\]/[0]/' "$root/state/compatible-core" > "$HOME/.local/lib/agent-relay/current/agent-relay-probe"
storage_refused --version 3.0.0
cp "$root/state/compatible-core" "$HOME/.local/lib/agent-relay/current/agent-relay-probe"
pass "matching version and status.get do not bypass the storage check"

# Simulate a format-changing core failing after it committed its migration.
# The real migration owns the same lock; this hostile boundary fixture also
# proves fallback never relies only on the installer's earlier snapshot.
require_disposable_home
rm -rf "$HOME/.agentworkforce/probe"
bash "$INSTALL" --version 2.1.0 > "$root/state/out" 2>&1 || fail "legacy fixture did not install"
make_legacy_storage
# The installer must create its lock privately even under the usual umask,
# and must not truncate an existing lock when it later opens it again.
QUEUE_FORMATS=0,1 MIGRATE_ON_START=1 release 3.4.0
(umask 022; bash "$INSTALL" --version 3.4.0) > "$root/state/out" 2>&1 || { cat "$root/state/out"; fail "owner could not migrate during service start"; }
[ "$(stat -c %a "$storage/delivery-queue.migration-lock")" = 600 ] || fail "migration lock was not created privately"
[ -s "$root/state/owner-migration-lock" ] || fail "owner never acquired migration lock"
printf 'preserved-lock-content\n' > "$storage/delivery-queue.migration-lock"
bash "$INSTALL" --version 3.4.0 > "$root/state/out" 2>&1 || fail "compatible same-version check failed"
[ "$(cat "$storage/delivery-queue.migration-lock")" = preserved-lock-content ] || fail "installer truncated migration lock"
pass "the starting owner acquires the migration lock before installer health checks, and lock creation is private and non-truncating"
require_disposable_home
rm -rf "$HOME/.agentworkforce/probe"
bash "$INSTALL" --version 2.1.0 > "$root/state/out" 2>&1 || fail "legacy fixture did not reinstall"
make_legacy_storage
QUEUE_FORMATS=0,1 MIGRATE_ON_START=1 release 3.3.0 broken
if bash "$INSTALL" --version 3.3.0 > "$root/state/out" 2>&1; then fail "broken migration core accepted"; fi
grep -q 'unsafe rollback refused' "$root/state/out" || { cat "$root/state/out"; fail "post-migration rollback not fenced"; }
[ "$(current)" = 3.3.0 ] || fail "old core was reactivated after migration"
[ -x "$HOME/.local/lib/agent-relay/versions/3.3.0/agent-relay-probe" ] || fail "compatible recovery binary deleted"
[ -x "$HOME/.local/lib/agent-relay/versions/2.1.0/agent-relay-probe" ] || fail "previous release deleted"
pass "failure after migration keeps the compatible binary and refuses unsafe fallback"

# The legacy-unit fallback has a different activation path: do not restore its
# ExecStart merely because it still answers with the earlier version.
require_disposable_home
rm -rf "$HOME/.agentworkforce/probe"
legacy="$HOME/.local/lib/agent-relay/legacy/agent-relay-probe"
mkdir -p "$(dirname "$legacy")"
cp "$HOME/.local/lib/agent-relay/versions/2.1.0/agent-relay-probe" "$legacy"
require_disposable_home
rm "$HOME/.local/lib/agent-relay/current"
require_disposable_home
printf '[Service]\nExecStart=%s relay serve --headless\n' "$legacy" > "$XDG_CONFIG_HOME/systemd/user/agent-relay.service"
echo 2.1.0 > "$root/state/running"
if bash "$INSTALL" --version 3.3.0 > "$root/state/out" 2>&1; then fail "broken legacy move accepted"; fi
grep -q 'unsafe rollback refused' "$root/state/out" || { cat "$root/state/out"; fail "legacy unit rollback not fenced"; }
[ "$(current)" = 3.3.0 ] || fail "legacy unit fallback changed current after migration"
grep -q '/current/agent-relay-probe' "$XDG_CONFIG_HOME/systemd/user/agent-relay.service" || fail "old legacy unit restored"
pass "post-migration failure cannot restore an incompatible legacy unit"

# Same-version product replacement sets the old binary aside in .replaced;
# that third automatic fallback must check its capabilities too.
require_disposable_home
rm -rf "$HOME/.agentworkforce/probe"
release 4.0.0
bash "$INSTALL" --version 4.0.0 > "$root/state/out" 2>&1 || fail "old product fixture failed"
QUEUE_FORMATS=0,1 MIGRATE_ON_START=1 RELEASE_PREFIX=Desktop-acme-7 RELEASE_ROOT="$root/releases-acme" release 4.0.0 broken
echo 4.0.0 > "$root/state/acme-latest"
if acme > "$root/state/out" 2>&1; then fail "broken same-version product accepted"; fi
grep -q 'unsafe same-version rollback refused' "$root/state/out" || { cat "$root/state/out"; fail "same-version fallback not fenced"; }
[ "$(product_of)" = Desktop-acme-7 ] || fail "old same-version product restored after migration"
[ -x "$HOME/.local/lib/agent-relay/versions/4.0.0.replaced/agent-relay-probe" ] || fail "previous product deleted"
pass "post-migration failure keeps both same-version products instead of restoring the old one"

# Only remove this test's fixtures before the independent cloud-feed suite.
require_disposable_home
rm -rf "$HOME/.agentworkforce/probe"
bash "$INSTALL" --version 2.1.0 > "$root/state/out" 2>&1 || fail "legacy fixture restore failed"
