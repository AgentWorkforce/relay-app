# People and their agents

The relay core's roster and people methods describe live work rather than a
history of every session the desktop has discovered. `sessions.list` marks a
local session `live` only while the provider is running or recently
reachable. The desktop apps no longer have My agents or Team pages: the
sessions table reads the roster for idle state, and agents and other clients
use the methods below.

## Person and avatar data

The signed-in Agent Relay Cloud member roster supplies each person's id, name,
email and optional `avatarUrl`. The UI keeps this roster in memory only. It
uses the account avatar when present, otherwise the standard lowercased-email
Gravatar URL. Logout clears the roster. The relay core serves it as
`people.list`; the signed-in person is `status.account.person`.

Local-session registrations carry their owner in Relaycast metadata:
`cloud_user_id` and `cloud_workspace_id`, which the engine uses to credit the
agent's messages to that person, and an `owner_hash` fingerprint for the
roster. The owner is the attached install's person; when it changes, stored
registrations are updated through `PATCH /v1/agents/{name}`. The desktop
retains only the hash while decoding `/v1/agents`, joins it to the
in-memory Cloud member roster for presentation, and never exposes it through
the private `GET /agents` socket. Registrations without an owner remain visible
under **Other live agents**; arbitrary metadata is never an authorization
boundary.

A live entry's canonical address is `agent@machine`.

## Recipient liveness and message state

What happened to a message is what Relaycast holds for it; whether its
recipient runs is only a hint beside it.

**Message state.** `GET /v1/agents/{name}` lists the deliveries Relaycast
still holds for the agent (`pending_deliveries`, oldest 50). The core reads
the sent message's row there:

| `message` | Relaycast |
|---|---|
| `queued` | its row is `queued`: not yet handed to the agent's host |
| `delivered` | its row is `delivered`: handed to the host, not yet acknowledged |
| `picked_up` | no row: the agent acknowledged (or refused) it |
| `no_such_agent` | no agent has the name or address |
| `null` | the record cannot tell: a server without `pending_deliveries`, an unknown row status, or a full list without the message |

**Liveness.** Every roster agent carries `liveness` from Relaycast's
presence and, for agents registered from this computer, its own session
discovery:

| `liveness` | Source |
|---|---|
| `live` | `status` `active` or `online` with `last_seen` within Relaycast's five-minute window; a session on this computer that is running |
| `idle` | `status` `idle`, `waiting` or `blocked` with `last_seen` within the window |
| `offline` | anything else; a desktop registration marked `idle` (its session ended); a session on this computer that is not running |

An agent without `address` has no node (SDK and MCP agents register
without one); that alone says nothing about whether it runs. A released
agent's name resolves to no agent. Relaycast expires `active` itself;
`idle`, `waiting` and `blocked` are self-reported and never expire there, so
the core applies the same window to them. A desktop registration
(`metadata.source` `relay-desktop`) is marked `idle` by its desktop when its
session ends and `active` when it runs again, so for it `idle` is `offline`
and an ended session is offered on no device. This computer's own sessions
are read from discovery.

**Workspace.** The roster is read with this computer's workspace key. It
answers for your messages only when your relay identity sends on the same
origin and the account does not use another workspace; otherwise
`roster.get` and `messages.send`'s `recipient` have `known: false` and say
nothing.

`roster.get {to|address, message_id?}` looks up one recipient and, with an
id, that message; `messages.send` answers with the same lookup read just
after the send. A send to no agent is `not_found` and nothing is queued.
`roster.list` without `include_idle` reads the running statuses (`active`,
`online`, `idle`, `waiting`, `blocked`, one request each: Relaycast filters by one
status) and lists the agents that are `live` or `idle`; with it, every agent
(`status=all`). A status whose read fails leaves out only its agents, is
named in `unread_statuses` and logged; the list fails only when every read
does.

macOS's Message an Agent sheet looks the recipient up as it is entered and
warns inline before queueing for one that is `offline` (**Queue Anyway**).
After a send it shows the message state (**Queued — agent not running** or
**Queued — not yet handed to the agent**, **Delivered**, **Picked up by the
agent**, **No such agent**, or **Sent — delivery unknown**), read again two
seconds after the send, on `sessions`, `roster` and `messages`
invalidations, and every 10 seconds while the sheet is open. Writing the next
message or closing the sheet drops the note. Beneath the composer it says
whether a reply is expected, with when the recipient was last seen.

The Linux app's Messages window does the same for its conversations: a
recipient is looked up when its conversation is shown and two seconds after a
send. On the same invalidations and every 10 seconds it is read for the
conversation shown and for any whose last message is not yet picked up or
refused, one read per conversation at a time, and no longer once the
conversation closes.

## What an agent is doing

Every roster agent may carry `activity`, derived by the desktop it runs on from
the session's own records; agents are never asked to report it
(`relay::activity`):

| Field | Source |
|---|---|
| `turn` | `working` or `idle`: Claude Code's `~/.claude/sessions/<pid>.json` `status`, the Codex rollout's running turn, or a spawned agent's harness (`waiting` on a permission counts as working) |
| `repo`, `branch` | the working directory's git checkout: the `origin` remote's name, else the checkout folder; the branch, or a short commit when detached. Read from git's files, cached 30 seconds |
| `doing` | the session title and the in-progress item of its own to-do list (Claude Code's `tasks/<session>/` or TodoWrite, Codex's `update_plan`), joined by ` · `, at most 120 characters, without Agent Relay, Relay Connect, invite or join links, links carrying a token or code, or workspace keys |
| `updated_ms` | when it last changed |

Transcripts are tail-read: the first look reads the last 2 MiB, later looks
only what was appended. Only sessions shared with the workspace say what they
do: those uploads include, which every session put on the relay is, and under
sharing `all` every registered session; a session someone stopped uploading
never does. Claude app chats, OpenClaw, Hermes and Pi sessions have no upload
identity and publish nothing.

The desktop publishes `activity` (or `null`) as Relaycast agent metadata on the
name an agent is shown by, only when it changes and while the agent runs: at
most once every two minutes per agent and six publications a minute per
computer, oldest registration first. Only a publication these limits approved
carries `activity`; a title change or rename sends its marker without it.
Publications ride the name markers that
carry titles, so they are retried when they fail and paused with every other
write while a workspace is at its plan limit or overloaded. An unchanged
activity is never rewritten (compare the v2026.10.48 journal flood); after a
restart each running agent's is sent once, under the same limits, so one an
earlier run published is replaced or cleared. Signing out stops sharing it.

`roster.list`, `roster.get` and the session socket's `GET /agents` include
`activity` only for a `live` agent; this computer's own agents show what the
core derived last, not what Relaycast was told. Cloud's `list_relay_agents`
and `list_team_activity` do not read it yet.

## Person inbox decision

Current Relaycast DMs and delivery acknowledgements belong to agents, not to a
workspace person. They cannot durably hold a message for a teammate with no
online agent, and agent registration metadata is not authoritative ownership.
[relaycast#457](https://github.com/AgentWorkforce/relaycast/issues/457) tracks
the required person principal, waiting/picked-up state and unread counts.

The chosen routing design is explicit recipient selection: a waiting message
stays in the person's inbox until they choose a live agent (or opt into a saved
preferred agent). “First agent online” is intentionally rejected because it
could deliver a private teammate message to unrelated automation. Until the
server contract ships, desktop shows the Inbox affordance disabled; it does not
pretend that an agent DM is a person inbox.

## Codex presence

macOS reads only user-facing `cli` and `vscode` threads, skips Relay-spawned
system-reminder titles, waits up to eight seconds for Codex's SQLite writer,
and reads extra rows before applying the automation filter. A thread is live
when a signed Codex resume process names it or its database row was updated in
the last five minutes. This prevents old threads and `exec` automation from
crowding live work out of the main list.
