# Local upload queue

`history.db` keeps captured provider evidence. `delivery-queue.db`, in the same install directory, keeps the upload journal: jobs, selections/exclusions, feed cursors, immutable batches, prepared wire bodies, retries, quarantine and counters. The public delivery open functions still accept the history path. Their returned SQLite connection owns the queue and attaches history read-only; capture uses the evidence store's own writer.

## Cutover

An install using the retired export journal first copies and verifies its upload state, auxiliary tables and immutable requests. After cutover it keeps every unresolved batch and prepared body, retires the old capture journal, and restarts feed cursors with revisions above the journal high-water mark. Old requests keep their identities and bytes; only their obsolete local feed-end positions become zero, so acknowledgments cannot skip new feed work. Retry deadlines, failures and exclusions survive.

Only the collector owner migrates an existing install. Control and status helpers refuse an unready queue rather than migrate or fall back. An old helper cannot resume writing the previous upload tables after cutover.

1. Under the evidence database's writer transaction, refuse only a batch with a current worker and an unexpired lease (expired or abandoned leases copy unchanged and recover on claim), inventory table definitions/keys/rows/byte digests/sequences/indexes/triggers and install durable write fences on the legacy upload tables. Re-read any existing witness inside that transaction before creating one. A separate file lock serializes concurrent cutovers; a contender gets a recoverable upgrade-pending result. Commit the frozen witness.
2. Create a private queue database, copy exact rows and rowids, restore autoincrement sequences, then install indexes and triggers. Verify the copy before committing its `copied` state. Triggers are installed after copying so counters do not increment twice.
3. Verify both sides against the witness. In one source transaction replace legacy upload tables with views that refuse old reads/recreation, and mark the source `done`.
4. Verify the queue again and mark it `upgrading`. Initialize the queue schema and, when needed, rebuild the retired journal without discarding requests. Only then mark it `ready`. Helpers refuse the intermediate phase; a crashed initializer resumes on the next owner open. Normal queue writes can now start.

Each committed phase is recoverable. A copied destination is never overwritten from a stale source. A missing/foreign queue, changed schema, changed counter/sequence or mismatched row digest refuses cleanup. If storage fills or a process stops before a commit, SQLite rolls back that transaction and the next owner open resumes. Keep both database files and their WAL files together when backing up an install. Downgrading to a binary that expects upload tables inside history is unsupported after cutover; it refuses instead of creating a second queue.

A live old lease blocks migration even if its deadline has passed: the installer must first stop/quiesce the old delivery worker. The migration does not steal leases or reinterpret an uncertain network result as acknowledged.

## Retry and retention

The move preserves batch identities and exact prepared bytes, including bodies prepared by older builds. It does not re-scrub or rewrite an existing identity. Destination compatibility checks remain responsible for refusing unsafe old bodies; this storage change must be composed with the reviewed scrubber/legacy-identity fix before release.

Existing bounded batching, leases, retry backoff, explicit retry and quarantine behavior use the queue. Receipt compaction removes settled rows only, retaining acknowledged rows beyond an unresolved gap. There is no time-based purge of unresolved or quarantined work. A full disk reports a local storage failure and cannot create an acknowledgment. A user-confirmed session reset removes both databases and their WAL/SHM files before recreating an empty queue.

The decommissioned-install marker observes both databases and WALs. A queue-only write invalidates it. Retained delivery workers and query readers also fence queue file replacement, independently of evidence replacement.

This is local accounting, not server indexing success: a queued remote receipt still must reach durable acceptance before the Desktop acknowledges its batch. Server database admission is a separate asynchronous pipeline concern.

## Installer compatibility

The Linux headless installer checks local format metadata before switching binaries, including explicit downgrades, `--rollback`, same-version replacements and automatic fallback after a failed start. It holds private queue migration locks around each compatibility check and binary switch, then releases them before restarting the core so the owner can resume migration. Downloads and health checks do not hold those locks. A frozen migration requires a compatible binary even before the destination file exists. An empty destination left before the first copy commits can resume; a destination with user objects but no migration marker refuses activation. Unreadable or unknown metadata refuses activation; it never triggers a reset or guard removal.

`agent-relay-probe --local-storage-capabilities` reports supported upload formats without opening history, reading credentials or contacting the core. Protocol 1 reports `upload_queue_formats`: `0` is the legacy in-history queue; `1` is the sidecar. The sidecar migration witness and advertisement share `storage_format::UPLOAD_QUEUE_FORMAT`. A binary without this command is unverified and accepted only for legacy state; this deliberately also refuses some older sidecar-capable builds rather than guessing from their release number. A failed update keeps the new compatible binary when its predecessor cannot read the migrated data, and instructs the user to install a newer compatible release.

This guard covers the Linux headless installer. GTK/Windows installers, macOS update/takeover paths, and migration-aware session UI messaging require the same compatibility contract; the capability advertisement alone does not qualify those paths. Release validation must exercise the actual prior-release-to-candidate upgrade with queued requests, interruption/restart and rejected incompatible rollback, then verify capture, durable upload and a normal hosted read. Core liveness or a matching version is not upload acceptance.
