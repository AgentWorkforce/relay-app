# Incremental local capture

The shared Rust collector implements this behavior on macOS, Linux and Windows.
The filesystem watcher attaches before historical reconciliation starts. Each
hot pass retains up to eight exact dirty paths and their provider scopes. A
persistent SessionStore handle advances catalog fingerprints through change-feed
deltas. Primary Claude/Codex JSONL acquisition bypasses the global sync-state map
and resumes existing metadata/record byte checkpoints.

An independent five-second safety poll checks at most 32 transcript stamps,
even when every native watch registered successfully: a streaming writer may
flush while keeping its descriptor open, and some backends defer modification
events until close. One catalog page supplies up to 32 recently catalogued
local files. Catalog stat slots rotate over that stable window, and skipped
slots retain their previous stamps instead of becoming fabricated first-seen
work. Up to eight of the stat slots rotate over at most 32 recently
admitted event-path hints; this retains empty create attempts until their first
parsable record arrives. Hints record their stamp before hot acquisition and
retry only after metadata changes. Catalogued hints transfer after stamp
comparison; missing or denied hints are removed, and newer hints evict older
ones. Admission uses the same validated sharing roots. Changed files enter the
same eight-path hot lane, even during unfinished cold work. This bounded recent
window does not guarantee low latency for every file: older/evicted paths,
uncatalogued files with no admitted event, database-only locators and nameless
sidecars rely on fair cold coverage. Selected inventory performs shallow hot
updates and keeps a separate cold deadline. At most 128 notices are folded
before each scheduling decision, so continuous event refill cannot prevent a
cold turn or checking stop.

Safety-only exact batches reuse independent SDK Reconciliation proof contexts
for the validated base and extra roots; they do not advance cold directory
positions. This is the same native Claude/Codex proof used by cold coverage,
including its identity, trusted-ctime, generation and destination safeguards.
Uncertain or unsupported cases retain ordinary acquisition. Merely changing
force=false is insufficient for legacy native evidence without a Session
hydration checkpoint. Real native notices and mixed native/safety batches keep
the existing forced route, while loss/directory/overflow recovery remains due.

The retained context key compares the SDK's complete ProviderRoots value,
including custom provider roots, plus the database path and complete extra-root
set. Root/consent changes and successful global maintenance discard these
contexts. A new extra root is discovered without adopting a stale proof set.
The SDK may call an exact attempt swept even when proofs skipped every file;
safety delivery wakes instead compare the complete before/after Watermark
(revision and epoch), retaining identity-only/non-session changes. Legacy state
may be loaded once per proof-context generation; its initial cost still scales
with that state, while repeated unchanged batches retain it. No native cursor
is copied into a Session checkpoint by this path.

Cold work retains the SDK Reconciliation iterator across slices: at most 64
directory entries and eight paths per turn. Providers and additional shared
Claude roots rotate fairly. A due cold slice gets a turn after at most one hot
pass; continuous active writes cannot starve historical coverage. Directory
changes, watcher loss and overflow of the 4,096 distinct pending-path set request
a fresh round after the current round finishes, preserving admitted hot paths.
Watcher notifications arrive on an unbounded channel; the pending-path cap and
128-notice scheduling turn do not bound queued-notification memory.
A periodic backstop reached during unfinished work does not synthesize lost
coverage or queue an immediate successor. Its next deadline is retained from
completion of the current round, so a quiet long round has a full post-completion
interval before periodic coverage starts again. Real loss, directory/rename and
overflow requests still retain an immediate successor without resetting the
current iterator. Without native inventory watches, completed shallow rounds
retain the shorter polling cadence. Failed round creation retains the request
and retries with pacing. A full request preserves queued exact paths and their
provider allowlist: those still drain in eight-path batches while reconciliation is deferred. When
only the full request remains, it waits for reconciliation without a hot
deadline. A successful refresh transfers it to the current or successor round.
Temporary initialization failures also retry.
A persistent enumeration failure remains queued in its SDK round, but does not
hold healthy coverage open forever. Once every ready iterator and acquisition
queue has drained, a partial pass starts the same fresh-coverage interval as a
completed pass: in broad mode 30 minutes while a complete, current native
watch is attached on macOS and Linux and 60 seconds otherwise, or the existing watcher-dependent
interval in shallow inventory. Actual loss still requests the immediate next
healthy pass. Failed obligations retain their ordinary retry deadline while
that interval runs; they are not deferred to the periodic backstop. The next
pass refreshes the retained SDK objects in place, including completed sibling
Claude roots, and adds newly authorized roots once by canonical projects path.
It never resets a still-active iterator or creates a second scanner for an
unresolved root. Partial passes do not increment completed sweeps, run full
successful maintenance, or finish startup progress as successful. Only a fully
completed round can enter those paths.
Folder settings are checked once a second; changed consent reopens retained
stores and discards old rounds and pending paths before more acquisition.
Base roots, extra Claude roots, hot capture, shallow inventory, recent-file
fallback and watcher admission use the same validated folder-choice snapshot.
New sibling folders can still be discovered between settings refreshes, but a
later malformed settings file cannot replace a recorded denial with default
sharing. Failed validation blocks acquisition until a valid refresh; remembering
running Claude folders cannot overwrite malformed choices.
Selected-only inventory uses the same scheduling with shallow discovery; it
never hydrates an unselected source. Selected evidence capture also rotates
through at most 32 file locators per ordinary one-second member poll, including
successfully watched files. A small selection therefore does not depend on a
close event; a large selection takes multiple fair turns. Immediate named
callbacks, first hydration/new members, real loss retries and the ten-minute
full member backstop retain their existing behavior and may compare more.
Membership loading/filtering and watcher reconciliation still scale with the
selection; the safety budget bounds distinct files compared, not the entire
selected-job round or initial hydration. Changed files may be resampled after
hydration by the existing stamp bookkeeping. Retained selected readers also
validate and rebind folder choices to the same RootSettings snapshot before
acquisition. Denied Claude locators never enter file stats or watcher
registration; targeted hydration receives only validated base/extra roots.
Malformed refresh blocks acquisition, while the cached valid snapshot retains
its denials during the refresh gap. Other providers retain their existing
selected locator rules and family hydration semantics.

A successful cold slice sleeps 50 times its elapsed work after subtracting the
SDK's measured SQLite busy-handler sleep, with no fixed minimum
and no upper clamp. Hot passes retain a 10-times-cost gap and their existing
minimum debounce/retry interval. Remote capture has its own transport/cadence
and 10-times-cost pacing. These are separate worker occupancy controls, not a
measurement or guarantee of aggregate CPU across all workers. Errors/lock
contention keep a nonzero retry floor. `cold_slices`, `cold_entries`, `cold_paths`,
`cold_slice_ms_last` and `cold_slice_ms_max` expose full elapsed slice time;
`cold_sqlite_busy_wait_ms_last` and `cold_sqlite_busy_wait_ms_total` expose the
known blocking component separately. Inventory diagnostics retain both values.
This avoids multiplying a five-second writer-lock wait into a 250-second gap.
It does not measure CPU or subtract unmeasured I/O or thread waits.
Cold reconciliation reuses existing native unchanged proofs where valid, so
already captured primary transcripts do not need new hydration checkpoints.
Parser, source identity, cursor, generation and available destination evidence
must agree; uncertainty takes the normal acquisition path. A legacy state with
no destination marker retains the native evidence-existence guard and cannot
independently detect partial historical row loss. Hot acquisition is unchanged.

The first completed round runs a full maintenance audit to seal global parser
generations and repair destination shortfalls. A successful audit postpones the
next for a day; contention, errors and preemption leave it due for a later round
with a nonzero retry gap. There is no daily cap on failed attempts.
Admitted watcher activity interrupts maintenance through the existing SDK stop
token; every watcher mode also checks the bounded recent-file window during an
audit, so a held-open writer can interrupt it without a native callback. The scheduler then services hot work. Committed per-file checkpoints
survive, while incomplete generation seals stay unpublished. Retrying an audit
can repeat discovery; continuous writes can postpone maintenance completion.
Each completed Claude/Codex source round also retries unresolved continuity once. Primary
explicit/affected continuity resolves immediately through indexed identities and
the changed event revision range. Full maintenance, unresolved recovery, sidecar
fallbacks and provider database snapshots can still scale with their inputs.
One large initial transcript/rewrite retains its existing atomic acquisition
boundary. These are directory/file-count bounds, not hard elapsed-time or byte
bounds for every provider. Cooperative stop reaches provider/file/record and
continuity-row boundaries; completed checkpoints survive restarts.

## Reproducible producer dependency

Both `ai-hist` and `history-provider-sources` in desktop-core/Cargo.toml are
pinned to the same exact RelayHistory release from crates.io. They must resolve
to the same version because provider helper arguments carry ai-hist types, so
move them together in one intentional dependency update.
