# Continue Claude from your phone

This path sends the signed-in computer owner's messages into an existing
Claude Code conversation as native user turns. Claude stays running on a Mac
or Linux computer. Its session ID, working directory, tools, model, and
permission configuration stay in place, and the desktop conversation receives
the same turns. It is on unless turned off.

## What it needs

1. Update Agent Relay Desktop and the iPhone/iPad app.
2. Use Claude Code 2.1.310 or later in the 2.1 release series.
3. Enable agent hosting and wait for the computer to be claimed by your signed-in
   account. Native turns use that verified ownership binding; until it exists,
   **Continue agents from mobile** in desktop Settings is off and says it
   is waiting for that.
4. Once the computer is claimed, Agent Relay installs its bundled Claude plugin
   in each watched Claude login folder that you share with Relay, including
   separate logins, at startup and on each discovery pass. New shared folders
   discovered later receive the plugin automatically. Start a new Claude
   session, or run `/reload-plugins` once in an existing session after its folder
   receives the plugin. Unshared folders are left alone. Sessions whose plugin
   could not be installed, or whose ID matches multiple native sessions, do not
   advertise native mobile continuation and get peer messages instead.
5. On the phone, open the existing agent conversation and send a message.

Turning **Continue agents from mobile** off cancels queued submissions,
clears the mobile status and leaves the inert plugin in place; turning it back
on checks the ownership claim and installs again. The one switch sets both
core settings, `mobile_claude_handoff` for Claude Code and
`mobile_native_continuation` for the other adapters; an agent changing
settings over the session socket may still set them separately.

Reading the conversation sends nothing. A message stays queued until Claude
can accept it; opening the phone app does not take over a busy turn. A delivery
receipt means native input was accepted, not that Claude has finished the task.
Tool approvals arrive in the same mobile conversation and apply to one action.
They expire after five minutes. Connection retries reuse the same prompt and
decision; they do not grant approval or create another prompt.
Answers and approvals stay bound to the account and workspace that started the
turn, including when a connection retry overlaps an account switch.
Claude's existing permission hooks still apply. Interactive `AskUserQuestion`
forms remain in the desktop UI in this version.

After your first mobile message, replies stay synced to your phone, including
replies to later messages you type in the same desktop conversation. Before
that first mobile message, desktop replies are not forwarded. Permission
prompts for turns you start on the desktop still use Claude's desktop UI.

Keep the computer awake and Claude running. Returning to its desktop window
requires no merge or new session. Channel posts, @mentions and messages to a
session without the plugin remain peer messages, and no one but the verified
owner can approve a tool request.
Disabling the setting cancels queued submissions and clears the mobile status.
An uncertain submission is never automatically replayed.
Restarting Agent Relay ends existing native control generations and their pending
approvals. Send a new mobile message to reconnect to the same Claude conversation;
no plugin reload is needed for a Relay restart. Previously submitted messages are
not replayed, and approvals from before the restart cannot authorize a later turn.
If login sharing settings cannot be read, native support pauses until they can be
verified again; Relay does not install into unknown folders or treat that outage
as an explicit unshare.

## How a message reaches the plugin

The plugin does not poll on a timer. When a session starts, it runs `curl`
once to hold a doorbell open on Agent Relay's private socket
(`GET /claude/mobile-handoff/watch`). The core adds a line to that response
whenever the session has a message waiting, and the plugin then collects it
over `poll`, which only Claude's own process can call. A ring carries no text,
so the doorbell can be held by a helper process. A message that arrives while
Claude is working is collected when that turn completes; while the doorbell is
open, the message waits as long as the turn takes. While a message waits and
Claude is idle, each of the delivery's retries rings again, so a ring the
plugin missed (a poll that failed, a turn that ended mid-poll, several messages
queued in one turn) costs at most one retry. A failed poll or answer is retried
with backoff from one second to a minute. Opening a doorbell alone does not
mark the plugin as loaded: Claude's own first poll does. The plugin reconnects with
backoff after Agent Relay restarts, and opens a new doorbell after `/clear` or
a resume. Without `curl` on the `PATH`, the plugin falls back to asking every
two seconds. A session that loaded the plugin before this change keeps asking
every two seconds until it restarts or runs `/reload-plugins`.

Long answers are shortened with a visible notice on mobile; their complete text
remains in Claude. Answers awaiting transport are kept in a bounded in-memory
plugin queue, so closing Claude while offline can lose undelivered mobile replies.

## Teammates' direct messages

With **Native input from teammates** on (the default), a direct message from a
teammate or one of their agents to a session whose plugin is loaded is handed
to Claude Code the same way, as a user turn, instead of through Claude Code's
cross-session inbox. Loaded means the plugin holds its doorbell open, or (a
plugin from before the doorbell) has polled the core within the last 30
seconds; installed files alone do not count, so a session opened before
the plugin landed keeps getting peer messages until it restarts or runs
`/reload-plugins`, rather than having a message wait out a lease and be
refused. The inbox labels a message as coming from another
session, may hold it for approval, and makes Claude refuse some requests, such
as editing CLAUDE.md; a user turn does none of that. While relay messages are
hidden (**Show me relay messages in full** off, the default), the turn shows the same
two-line pointer a peer message gets: `[Agent Relay native input — from @name ·
ref …]` and the `read` command that prints the message, its reply
instructions and the peer authority note. With relay messages shown, the turn
carries the whole message with its attribution and reply instructions. Either
way the session knows who is asking and answers them itself. It is queued under the owner's binding, so a tool approval
it raises still goes to the owner's phone, never to the sender. Its answer is
not sent to the owner's phone (the poll action carries `forward_answer: false`
and the plugin forwards nothing for that turn), and it does not engage the
owner's reply subscription. Turning the setting off cancels a teammate's
message not yet started and sends later ones through the peer inbox again.

## Platform boundary

Claude connects directly through its native HTTP API to Agent Relay's private
Unix socket. The core requires the exact registered Claude process; tool
subprocesses cannot access the native-input bridge. The installed bridge config
contains only the socket path, never a helper executable or credential.

Windows keeps the setting visible but disabled, with an explanation. Its core
reports native continuation unsupported and refuses opt-in before installation.
Claude's native connection must be verified with Windows named pipes before the
feature can be enabled there. No subprocess fallback is provided.

## Validation

- `node --test scripts/test-claude-mobile-plugin.mjs` checks native submission,
  retry safety, conversation identity, permission decisions, and answer forwarding.
- Rust tests cover the durable queue, current owner and process authorization,
  session-socket routes, installer, opt-out, and scoped approval replies.
- **Mobile CI** runs the iOS unit tests and `MobileHandoffUITests` on a simulator.
  Its artifact includes screenshots of queued, connecting, ready, and failed states.
- `python3 scripts/test-claude-handoff.py --run --claude /absolute/path/to/claude`
  starts a disposable real Claude session on Unix. It tests desktop input, the
  bundled plugin's native user input, a delayed permission denial, and desktop
  input again, preserving the same session, process, model, directory, and tools.
  This opt-in test uses billable provider turns. Its local fake bridge replaces
  authenticated transport; it does not prove Cloud authentication or the mobile UI.

## Headless transfer is separate

This version does not fork, background, or merge Claude sessions. Testing native
`/background` on Claude 2.1.310 produced a different session ID and failed to
preserve an explicit tool restriction. Automatic transfer remains excluded until
the provider can preserve those constraints and support a verified return path.
