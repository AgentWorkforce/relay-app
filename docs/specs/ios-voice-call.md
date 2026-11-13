# Voice calls with an agent (iPhone and iPad)

Status: draft spec, updated October 28, 2026. The main call experience is
OpenAI **GPT-Live** (`gpt-live-1`) as the voice layer, with the person's Agent
Relay agent as its backend (milestone 2). The on-device path (Tier 0, Apple
speech recognition and voices) is milestone 1, implemented behind the
`RelayVoiceCallEnabled` flag in `ipad/native` (PR #708), and stays only as an
automatic fallback: calls always use GPT-Live, and the person cannot choose
on-device voice (decided October 28, 2026, because the on-device voices sound
too poor to offer as a choice). Both paths sit behind the same flag. See
[Milestones](#f-milestones-and-recommended-default).

## Goal

Let a person hold a hands-free voice conversation with any reachable Agent Relay
agent from the mobile app: speak, hear a short spoken answer, interrupt it, keep
going. The agent is the person's *real* agent, with its own session, tools,
memory and project, not a fresh context-free assistant.

The idea comes from [claude-call](https://github.com/caiovicentino/claude-call)
(MIT): mic → voice activity detection → speech-to-text → a warm, persistent
Claude Code session as the brain → sentence-by-sentence text-to-speech →
speaker, with a wake word or push-to-talk, barge-in with echo cancellation,
idle and turn timeouts, sound cues and a spoken-style system prompt. We reuse
the architecture, not the code: claude-call is a Python/pipecat desktop process
that owns the `claude` CLI; our app is a native iOS client of Agent Relay, and
the brain already runs somewhere else.

## (a) How it fits the app

### Where the brain runs

The brain is the agent the person is already talking to in **Messages** or
**Agents**. It keeps running on its own machine (a Mac, a Linux box, a cloud
node) inside the session the desktop broker manages. The phone never runs a
model and never starts a second copy of the session. Both call paths below
reach that same agent the same way: as Relaycast DMs from the person's
messaging identity.

### Main path: GPT-Live voice, Relay agent backend (M2)

GPT-Live is a full-duplex voice model that handles the spoken conversation
(listening while speaking, turn-taking, acknowledgements, paraphrasing) and
**delegates** reasoning and tool use to a backend. We use its **client
delegation** mode, where the application runs the backend: our backend is the
Relay agent. GPT-Live never gets the agent's tools or files; it only hears the
person and speaks what the agent answers.

```
 iPhone (WebRTC)                     OpenAI GPT-Live              Relaycast → agent
 ───────────────────────────         ─────────────────            ─────────────────
 mic track ───────────────────────▶  listens, speaks,
 speaker ◀──────────────────────────  full duplex
 data channel "oai-events":
   ◀── session.input_transcript.delta / output_transcript.delta   (captions, context)
   ◀── session.delegation.created {delegation.id, target: client}
        app builds the request from the person's transcript,
        runs the destructive-intent guard ─────── POST /dm ──────▶ live agent session
   ──▶ session.thinking.append  "Sent to the agent."                     │
        reply arrives (push + poll, later the event socket) ◀────────────┘
   ──▶ session.commentary.append {delegation_id, reply}   GPT-Live paraphrases it aloud

 Session setup (once per call):
 app ── SDP offer + Cloud credential ──▶ Agent Relay Cloud voice broker
         broker ── POST /v1/live/sessions (project key, session config) ──▶ OpenAI
 app ◀── SDP answer + session id ─────── broker
```

1. **Setup.** The app creates an `RTCPeerConnection` with the microphone track,
   a remote audio track and a data channel labelled `oai-events`, then sends its
   SDP offer to the Agent Relay Cloud voice broker with its existing Cloud
   credential. The broker checks the account, consent and minute quota, builds
   the session configuration itself (`model: "gpt-live-1"`, the live prompt,
   `delegation: {type: "client"}`, `store: false`, recent conversation text as
   `session.input`), and calls `POST /v1/live/sessions` with the OpenAI project
   key and `transport: {type: "webrtc", sdp}`. It returns only the SDP answer
   and the session id. The app sets the answer and waits for `session.started`.
   It must not send `session.start`; the HTTP request starts the session.
2. **Conversation.** Audio flows directly between the phone and OpenAI on the
   media tracks; the broker is not in the audio path. The app keeps the
   `session.input_transcript.delta` and `session.output_transcript.delta`
   fragments as captions and as context for delegation.
3. **Delegation.** `session.delegation.created` carries only metadata
   (`delegation.id`, `target: "client"`, `offset_ms`), not the request text. The
   app takes the person's transcript since the previous delegation (plus recent
   context for short replies like "yes, do that"), runs the destructive-intent
   guard, and sends it to the agent as a DM with the voice header. It sends
   `session.thinking.append` ("Sent to the agent.") with that delegation id, so
   GPT-Live knows the task is running without being told to say anything. When
   the agent replies, the app sends the speech-cleaned reply with
   `session.commentary.append` and the same delegation id; GPT-Live paraphrases
   it aloud. Each append is limited to 500 tokens, so long replies are trimmed
   to their opening and end with "The full reply is in the chat." A follow-up
   message from the agent becomes another append for the same delegation.
4. **Ending.** The app sends `session.close`, waits for `session.closed`
   (final `usage.seconds`), and reports the usage to the broker. Since GPT-Live
   bills silence too, the app closes the GPT-Live session after 60 seconds with
   no speech and no pending delegation. The call screen then shows **Resume**:
   a tap, or the on-device voice detector, starts a new session seeded with the
   saved text history. The Relay agent keeps working while the voice session is
   closed.

GPT-Live references (read October 28, 2026):
[getting started](https://developers.openai.com/api/docs/guides/live) ·
[delegation and tools](https://developers.openai.com/api/docs/guides/live-delegation) ·
[WebRTC](https://developers.openai.com/api/docs/guides/voice-webrtc?api=live) ·
[managing sessions](https://developers.openai.com/api/docs/guides/live-conversations) ·
[server-side controls](https://developers.openai.com/api/docs/guides/voice-server-controls?api=live) ·
[costs](https://developers.openai.com/api/docs/guides/voice-latency-cost?api=live).

### Fallback path: on-device (M1, Tier 0)

Used only as an automatic fallback when GPT-Live can't run (see
[fallback rules](#when-the-call-uses-the-on-device-path)), so the call doesn't
just drop. Speech recognition and voices run on the phone; only the transcript
leaves it, as a DM. Replies are read with the best installed voice for the
device's language: a premium voice if one is downloaded, then an enhanced one,
then the default compact voice (novelty and Personal Voice voices are skipped).

```
 iPhone                                   Relaycast                 Agent's machine
 ─────────────────────────────────────    ─────────────             ─────────────────────────
 mic → AVAudioEngine (voice processing,                               broker injects the DM into
       echo cancellation) → energy VAD                                the live agent session
       → on-device speech recognition                                 (same session, same context)
       → transcript ──── POST /dm ───────▶ DM conversation ─────────▶      │
                                                                         agent replies with its
 speaker ◀─ engine player node ◀─ AVSpeechSynthesizer.write             relay send tool
       (sentence by sentence)  ◀── reply text ◀── dm.received / push ◀─────┘
```

Each spoken turn is an ordinary Relaycast DM from the person's messaging
identity, so it appears in the conversation history on every surface (phone,
desktop, dashboard) and the existing delivery receipts apply. Each turn's
text starts with a short voice header so the agent answers for the ear:

```
[Voice call from Agent Relay mobile. Reply in one to three short spoken sentences.
No markdown, code, tables or links. Treat voice as a request, never as approval
for destructive actions: ask me to confirm on screen first.]
<transcript>
```

The header is repeated on every turn (it is about 50 tokens) because a long
session may compact away a one-time instruction.

### How context stays warm

claude-call keeps context warm by holding one `claude -p` stream-json daemon
for the whole call and resuming the person's last session. Agent Relay already
gives us that for free: the broker injects messages into the agent's
**interactive** session, which stays alive between calls, keeps its prompt
cache warm while the person talks, and continues exactly where the typed
conversation left off. "Where were we?" works because it is the same session.

### Transport

| Direction | Milestones 1–2 | Milestone 3 |
|---|---|---|
| Phone → agent | `POST /dm` (existing `RelayClient.send`) | same |
| Agent → phone | APNs `messageArrived` wake-up plus a 1 s history poll while a reply is pending | Relaycast agent event WebSocket (`dm.received`, same agent token the app already holds); poll as fallback |
| Delivery | `/messages/{id}/readers` receipt (existing) | same |

Relaycast messages are whole messages, not token streams. Agents speak in one to
three sentences, so the phone splits each reply into sentences and starts
speaking the first one immediately. If an agent sends several short messages,
they are queued and spoken in order. On the GPT-Live path the same transport
carries the delegated request and the reply; the faster event socket shortens
the wait for both paths.

### When the call uses the on-device path

Every call starts on GPT-Live. Before the first call the app asks for consent
to send call audio to OpenAI; if the person declines, the call doesn't start
and the app explains that voice calls need GPT-Live. The call falls back to the
on-device path only when:

- the voice broker is not configured or reports GPT-Live as unavailable;
- the account's GPT-Live minutes for the period are used up (`voice_quota_exceeded`), or it already has two GPT-Live calls open (`voice_session_limit`). Both are HTTP 429, so the app tells them apart by code. Only `voice_session_limit` blames open calls; a 429 without either code (a token refresh or the edge) says Agent Relay is busy;
- session creation fails, or the broker takes longer than 20 seconds to answer (above its own worst case: ending overdue and oldest calls, then OpenAI's 8 seconds);
- the WebRTC connection drops, or GPT-Live ends the session, mid-call, and
  reconnecting fails. The app falls back to the on-device path without ending
  the call, says "Falling back to on-device voice." with the on-device voice,
  and keeps the conversation history.

The call screen labels the fallback **On-device voice (fallback)**. There is no
menu to switch voices, and the next call tries GPT-Live again: tapping **Call**
while a fallback call is running ends it, waits for it to release the audio,
and starts a new call, so the app
never re-shows why an earlier call couldn't use GPT-Live. Earlier Debug
builds let people choose on-device voice; that stored choice
(`relay.voiceCall.preferOnDevice`) is removed and ignored. A call keeps
going in the background and on the lock screen (see
[Keeping a call alive](#keeping-a-call-alive)). Only real silence closes a
GPT-Live session: after 60 seconds with nothing pending, the call is put on
hold. **Resume** in the app or in the system call UI reconnects, and an agent
reply that arrives while the call is on hold reconnects it automatically to
speak the reply. When another call, Siri or an alarm interrupts, the session is
put on hold and resumes afterwards.

GPT-Live's WebRTC stack uses its own echo-cancelling audio unit, so the app
tears down one path's audio before starting the other; they never run at the
same time.

### Not in scope

Group calls, channels, calling a person, voicemail, and calls initiated by an
agent. The desktop apps (macOS, Linux) are unchanged: they do not have a voice
call feature and this spec does not add one.

## (b) Cost per call-minute

All prices were read from the vendor pricing pages on **October 27, 2026**
(links below). Assumptions for one call-minute: the person speaks ~30 s in
~3 turns, the agent speaks ~30 s (~75 words, **~450 characters** of TTS).
"VAD-gated" means the phone only streams audio while someone is talking.

### Speech in and out

| Tier | STT | TTS | Vendor cost / call-minute |
|---|---|---|---|
| **0 — near-free (default)** | Apple on-device recognition (`SFSpeechRecognizer` with `requiresOnDeviceRecognition`; `SpeechAnalyzer`/`SpeechTranscriber` on iOS 26) | `AVSpeechSynthesizer` (on-device voices) | **$0.000** |
| 1a — cheap | Deepgram Nova-3 streaming, $0.0048/min (VAD-gated ≈ 0.5 min → $0.0024) | Deepgram Aura-2, $0.030/1k chars → $0.0135 | **≈ $0.016–0.018** |
| 1b — cheap | Groq `whisper-large-v3-turbo`, $0.04/hour, 10 s minimum per request (3 × 10 s → $0.0003) | Cartesia Sonic, 1 credit = 1 char; Startup $49/1.25M credits ($0.039/1k) → $0.018; overage $45/1M → $0.020 | **≈ $0.018–0.021** |
| 1c — cheap | OpenAI `gpt-4o-mini-transcribe`, $0.003/min (gated → $0.0015) | ElevenLabs Flash/Turbo, $0.04/1k chars → $0.018 | **≈ $0.020** |
| 2 — premium voice | ElevenLabs Scribe v2 Realtime, $0.39/hour → $0.0065 (or Deepgram Flux English $0.0065/min) | ElevenLabs v3 or Multilingual v2, $0.08/1k chars → $0.036 | **≈ $0.043** |

Notes: Deepgram labels its streaming STT rates as limited-time promotional
pricing. Groq's whisper is batch (one request per utterance), which adds a
round trip after the person stops talking; Deepgram and ElevenLabs stream.
OpenAI's `gpt-4o-mini-tts` is priced per token ($0.60/1M text in, $12/1M
audio out) and its pricing page gives no per-minute figure, so it is not
costed here.

### Speech-to-speech realtime APIs

| API | Published price | Per call-minute (our assumptions) |
|---|---|---|
| Gemini `gemini-3.8-live` / `gemini-3.1-flash-live-preview` | audio in $3/1M tokens or $0.005/min; audio out $12/1M or $0.018/min; text in $0.75/1M, text out $4.50/1M | ≈ $0.005 + $0.009 = **≈ $0.014**, plus text/context tokens |
| OpenAI `gpt-realtime-2.1` | audio in $32/1M tokens, cached $0.40/1M, audio out $64/1M | not published per minute; flagship audio-out rate is 5× Gemini's |
| OpenAI `gpt-realtime-2.1-mini` | audio in $10/1M, cached $0.30/1M, audio out $20/1M | not published per minute |
| OpenAI `gpt-live-1` | $0.05/min billed per second, backend model and tools extra | **≥ $0.05** |
| ElevenLabs Agents | $0.08/min ($0.16 burst) | **≥ $0.08** |

**Decision: never use a speech-to-speech model as the brain; use GPT-Live as
the voice layer only.** A speech-to-speech model acting as the brain would be a
second, context-free model talking *about* the person's agent, which is what
claude-call (and we) set out to avoid. GPT-Live is designed the other way
round: it handles the conversation and delegates reasoning and tools to a
backend, and with client delegation that backend is our Relay agent. Gemini
Live is cheaper per minute on published rates, but we would have to build the
delegation layer ourselves on its function calling; GPT-Live has it built in
(decided October 27, 2026).

### GPT-Live main path

GPT-Live voice sessions cost **$0.05 per minute, billed per second**, and
backend model and tool usage is billed separately. The duration is **wall-clock
session time**: OpenAI counts time "when the user speaks, the assistant speaks,
both are silent, or the backend is working". Muting does not stop billing. A
WebRTC session bills 15 seconds at creation, which is credited against the
session's duration, not added to it. Our backend is the Relay agent, so there
is no OpenAI backend cost; the agent's own turns cost what typed messages cost.

| Item | Cost |
|---|---|
| GPT-Live voice layer | **$0.05 per call-minute** (wall clock, including waiting for the agent) |
| OpenAI backend model and tools | $0 (client delegation to the Relay agent) |
| Relay agent | its existing plan or key, same as typed messages |
| Voice broker | Agent Relay Cloud request handling: one session-creation call per call or resume |
| 5-minute call | **$0.25** |
| 100 call-minutes per person per month | **$5.00** per person |
| On-device fallback (Tier 0) | $0 |

Because silence is billed and coding agents can take 10–60 s to answer, closing
the GPT-Live session after 60 s of inactivity (see step 4 above) matters more
than any per-minute rate. A resume costs one new session; the 15 s billed at
initialization is credited, so it adds no extra charge.

### Brain

In Tier 0 the brain is the person's existing agent session, so **voice adds no
model cost for Agent Relay**: a spoken turn costs what the same typed message
costs on the agent's own plan or key (plus ~50 tokens of voice header).

GPT-Live now fills the "voice front" role: it answers conversationally at once
and delegates real work to the agent. The table below is what an alternative
front built on Claude would cost per call-minute, kept for comparison and as an
option if GPT-Live is not approved: 3 turns, a 20k-token cached prefix
(spoken-style rules + session summary + recent transcript), 150 new input and
300 output tokens per turn (reply plus low-effort thinking). Prices from
Anthropic's pricing page.

| Model | Input / output per 1M | Cache write (5 m) / hit per 1M | Per call-minute | One-time cache write |
|---|---|---|---|---|
| Claude Haiku 5.5 (≤100k prompt) | $0.10 / $0.50 | $0.125 / $0.01 | **≈ $0.001** | $0.0025 |
| Claude Sonnet 5.5 | $2 / $10 | $2.50 / $0.10 | **≈ $0.016** | $0.05 |
| Claude Opus 5.5 | $4 / $20 | $5 / $0.20 | **≈ $0.032** | $0.10 |

The minimum cacheable prefix is 286 tokens on all three, so a 20k prefix always
caches. Caching is what makes the front cheap: without it each Haiku turn would
pay $0.002 of uncached input instead of $0.0002. Keep the prefix byte-stable
(no timestamps), append the transcript, and use the 1-hour cache write
($0.20/1M on Haiku) only if calls regularly pause longer than five minutes.

### Why headless `claude -p` is a cost trap

claude-call's brain is `claude -p --resume <session>`. For us that would be the
wrong design even before price:

1. **It duplicates the session.** The agent is already running. A resumed
   headless copy re-reads the whole context; a 150k-token coding session on
   Opus 5.5 pays $0.75 to write the 5-minute cache (150k × $5/1M) every time it
   cold-starts or the cache expires between turns, then $0.03 per request to
   read it (150k × $0.20/1M). An agentic turn usually makes several requests
   (tool calls), so a busy minute can cost $0.10–0.30 *on top of* the session
   that is already running.
2. **It defaults to the most expensive settings.** claude-call ships with the
   Opus model at `xhigh` effort and `--dangerously-skip-permissions`.
3. **Its billing rules have moved twice this year.** Anthropic announced that
   `claude -p` and the Agent SDK would move from plan limits to a separate
   monthly credit on June 15, 2026, then paused that change; the October 7,
   2026 support note says `claude -p` signed in with a Claude plan still draws
   from plan limits, while with a Console API key it bills API rates. A
   product cost model should not depend on that.
4. **It can act without anyone watching.** Headless mode with skipped
   permissions lets a misheard sentence run tools.

Injecting the transcript into the agent's live interactive session avoids all
four.

Sources: [GPT-Live model and pricing](https://developers.openai.com/api/docs/models/gpt-live-1) ·
[GPT-Live costs](https://developers.openai.com/api/docs/guides/voice-latency-cost?api=live) ·
[Deepgram pricing](https://deepgram.com/pricing) ·
[Groq speech-to-text](https://console.groq.com/docs/speech-to-text) ·
[OpenAI pricing](https://developers.openai.com/api/docs/pricing) ·
[Cartesia pricing](https://cartesia.ai/pricing) ·
[ElevenLabs API pricing](https://elevenlabs.io/pricing/api) ·
[Gemini API pricing](https://ai.google.dev/gemini-api/docs/pricing) ·
[Anthropic pricing](https://platform.claude.com/docs/en/about-claude/pricing) ·
[Claude support: Agent SDK and `claude -p` usage](https://support.claude.com/articles/15036540).

## (c) Latency to the first spoken word

Target: the person hears *something* within 1.2 s of finishing a sentence, and
the agent's first sentence as soon as it exists.

| Step | Tier 0 budget | Notes |
|---|---|---|
| End-of-speech detection | 700 ms | Silence hangover; configurable 500–1000 ms (claude-call uses 1.0 s) |
| Final on-device transcript | ≤ 150 ms | Partials stream during speech; we only wait for the final |
| "Heard you" cue | ≤ 50 ms | Local tone through the engine; **first audible feedback ≈ 0.75–0.9 s** |
| `POST /dm` | 150–300 ms | One HTTPS round trip on a warm connection |
| Broker injects into the session | 0.3–1 s | Depends on the agent being idle |
| Agent produces its reply | 2–20 s | A coding agent's turn; not under our control |
| Reply reaches the phone | ≤ 1 s (poll) / ≤ 200 ms (socket, M3) | |
| First TTS audio | ≤ 150 ms | `AVSpeechSynthesizer.write` renders the first sentence locally |

Honest conclusion: with the real agent as the brain, **1.2 s to the
acknowledgement is met; 1.2 s to the agent's content is not**, because the
agent's own turn dominates (claude-call has the same property and covers it
with fillers). Milestone 1 plays the cue, shows "Waiting for <agent>…", and
after 4 s says "Waiting for the agent." once.

On the GPT-Live path the voice model handles turn-taking itself and speaks
right away, either answering conversational turns directly or acknowledging a
delegated task while the agent works. It can also keep talking with the person
and take extra details while the task runs. This is what makes the call feel
like a call. OpenAI's docs we read give no latency figure, so M2 measures, per
turn: end of speech → first GPT-Live audio, and delegation created → agent
reply → commentary appended → spoken. The agent's own turn still bounds the
time to its content.

## (d) iOS specifics

- **Audio session.** `AVAudioSession` category `.playAndRecord`, mode
  `.voiceChat`, options `.defaultToSpeaker`, `.allowBluetoothHFP` and
  `.allowBluetoothA2DP`; activate on call start, deactivate with
  `.notifyOthersOnDeactivation` on end. Prefer a 16 kHz-friendly input but use
  the hardware format the engine gives.
- **Echo cancellation and barge-in.** Enable voice processing on the engine's
  input node (`setVoiceProcessingEnabled(true)`), the same AEC FaceTime uses.
  AEC only cancels audio played *through the same engine*, so TTS must not use
  `AVSpeechSynthesizer.speak`: we render utterances to PCM with
  `AVSpeechSynthesizer.write(_:toBufferCallback:)` and schedule them on an
  `AVAudioPlayerNode` in the engine. Barge-in then works on the speaker: speech
  energy above the threshold for 250 ms while the agent is talking stops
  playback. Without voice processing (or if it fails to start) the call falls
  back to half-duplex: the mic is ignored while the agent speaks plus a 0.6 s
  tail.
- **GPT-Live transport (M2).** WebRTC, which OpenAI recommends for mobile.
  Apple ships no WebRTC framework, so the app adds a build of Google's
  BSD-licensed libwebrtc as a Swift package binary (to be chosen in M2; add it
  to `ThirdPartyNotices.txt`). The WebSocket transport is not an option for the
  phone because it authenticates with the project API key. libwebrtc manages
  its own voice-processing audio unit (echo cancellation) and audio session
  configuration, so the M1 `AVAudioEngine` is stopped before a GPT-Live session
  starts.
- **Speech recognition.** iOS 17–25: `SFSpeechRecognizer` with
  `requiresOnDeviceRecognition = true` when `supportsOnDeviceRecognition`;
  one request per utterance (the API limits request length). iOS 26+:
  `SpeechAnalyzer` + `SpeechTranscriber` (on-device, long-form, system-managed
  model) in M3. If on-device recognition is unavailable for the locale we tell
  the person and do not silently fall back to Apple's server recognition.
- **Endpointing.** Energy VAD on the (echo-cancelled) input buffers with a
  start threshold of 120 ms and a 700 ms silence hangover, combined with the
  recognizer's final result. Silero VAD via Core ML is an M3 option if energy
  VAD proves noisy.
- **Modes.** Open mic (default, like a phone call) or push-to-talk (hold the
  big button). No wake word in M1: on a phone the call screen is the explicit
  activation, and always-listening wake words raise review and battery
  questions.
- **Sound cues.** Short synthesized tones (listening, heard, ended) rendered into
  the same player node, so they are echo-cancelled too.
- **Interruptions.** Observe `AVAudioSession.interruptionNotification` (phone
  call, Siri, alarm): pause listening and speaking, then resume on `.ended`
  with `.shouldResume`. Observe `routeChangeNotification` (AirPods in/out) and
  restart the engine with the new format. `mediaServicesWereResetNotification`
  ends the call with a message.
- **Background and lock screen.** See [Keeping a call alive](#keeping-a-call-alive).
  Calls use CallKit for outgoing calls only (no PushKit; nothing rings), which
  gives the lock screen and Dynamic Island call UI, the green call indicator,
  mute, hold and end controls, and call audio priority. CallKit is not used
  where Apple does not allow it (China); there the app's background audio mode
  keeps the call running without the system call UI.
- **Bluetooth and AirPods.** HFP gives a mic but low-quality output; A2DP gives
  high-quality output without a mic. With `.allowBluetoothHFP` iOS picks HFP
  when the AirPods mic is used; that is the right default for a call.
- **Permissions.** `NSMicrophoneUsageDescription`: "Agent Relay uses the
  microphone when you start a voice call with an agent." and
  `NSSpeechRecognitionUsageDescription`: "Agent Relay turns what you say in a
  voice call into text on this device and sends that text to the agent."
  Ask only when the person taps **Call**.
- **App Review.** Usage strings must match behavior; background audio may only
  be declared once it is used. On the GPT-Live path call audio goes to OpenAI,
  so the microphone string must say so, the privacy policy and App Store privacy
  answers must disclose it, and the app asks for consent before the first
  GPT-Live call (guideline 5.1.2); declining means no voice call. The
  agent voice is synthetic and the screen names the agent being called.
  Nothing here changes the current draft release blockers.

### Voices and personas

The call's **Voice options → Voice and style…** sheet picks a persona and a
GPT-Live voice. Personas: **Standard**, **Pirate**, **Noir detective**,
**Hype-man** and **Calm narrator**. They change tone, pace and wording only;
the agent's answers, the destructive-action confirmation and the delegation
rules stay the same. The app sends only the persona id and voice name; the
Cloud voice broker validates them and adds its own preset style text to the
live prompt, so the app still cannot inject prompt text.

GPT-Live voices (from the `session.audio.output.voice` reference, October 28,
2026): alloy, ash, ballad, beacon, bossa, cedar, cinder, coral, delta, echo,
gleam, marin (default), meridian, quartz, ripple, sage, shida, shimmer, stone,
tempo, verse, vesper, willow. Each persona suggests one (pirate → ash, noir →
cedar, hype-man → verse, narrator → sage, standard → marin) until the person
picks a voice. The voice is fixed when a GPT-Live session starts, so changing
voice or persona mid-call reconnects with the conversation so far. On-device
voice approximates a persona with speaking rate and pitch.

### Keeping a call alive

Requirement (October 28, 2026): a call must not drop when the app
changes. What the app does:

- **The call belongs to the app, not a screen.** `VoiceCallService` owns the
  call, its audio and its GPT-Live connection. Closing the call screen
  (**Minimize**), opening another conversation or tab, or SwiftUI rebuilding
  views leaves the call running, with a green call bar to return to it.
- **Background and lock screen.** CallKit runs the call as a system call, and
  every build declares the `audio` background mode, so audio, recognition,
  GPT-Live and the reply polling keep running with the app in the background or
  the device locked. (A build script adds the background mode whenever
  `RelayVoiceCallEnabled` is on, which since build 47 is every build; the
  TestFlight workflow checks the archived app declares it.)
- **Route changes** (AirPods, Bluetooth, the speaker) are handled by the audio
  stack: libwebrtc for GPT-Live; the restarted engine for on-device voice.
- **CallKit.** The app has one CallKit provider, as Apple expects, and each
  action goes to the call that owns its ID; an action for an earlier call, or
  the end action a call sends itself when hanging up, doesn't end the current
  one. When iOS ends or resets a call, the call screen says so ("iOS ended the
  call", "iOS reset the call"). Each step of a call is logged for Console.app
  (subsystem `com.agentworkforce.relayipad`, category `voice-call`, one tag per
  call), and usage reports run as background work, with any left over sent when
  the app returns to the foreground.
- **Interruptions** (a phone call, Siri, an alarm) put the call on hold instead
  of ending it, and it resumes afterwards. No call outlives its use: a call
  ends after 5 minutes of silence, 15 minutes on hold with on-device voice, or
  15 minutes stuck in any other state, including at the consent question or
  the microphone prompt. A held GPT-Live call is closed to save minutes and,
  like any closed GPT-Live session, ends after 5 minutes.
- **Every GPT-Live session is reported.** A session holds one of the account's
  two GPT-Live slots until its usage is reported, so the app records each
  session on the device the moment the broker grants it, with its billed
  seconds as they update, and keeps it until the broker accepts a report. A
  call that ends reports at once; a failed report is retried every 30 seconds
  and whenever the roster loads; a session left open by a killed, crashed or
  updated app is reported on the next launch. A call that replaces its
  session (a new voice or persona, resuming from hold, reconnecting) waits
  up to 3 seconds for the old session's report before asking for the next
  one, so the two don't hold both slots at once; a report that doesn't land
  in time is retried as above. The broker's reaper ends any session no
  report reaches.
- **Brief network loss.** A dropped GPT-Live connection reconnects
  automatically with a new session seeded with the call so far: retries at
  0.5, 1.5, 3, 6, 10 and 10 seconds (about 30 seconds in total). After that the
  call continues with on-device voice. Requests already sent to the agent are
  unaffected, and their replies are spoken after reconnecting.
- **Relaunch.** While a call is active the app saves the agent, recent call
  text and the messages already seen. If the app starts again within
  15 minutes, it rejoins the call automatically once the agent is on the roster,
  and reads any reply that arrived in between.

What iOS cannot keep alive, and the mitigation:

| Case | Why | Mitigation |
|---|---|---|
| Installing a new build (Xcode Run, TestFlight or App Store update, reinstall) | iOS terminates the running process to replace its binary; nothing in the old process survives | The call rejoins automatically on the next launch (within 15 minutes). The agent's work is unaffected because it runs on its own machine. Expect a few seconds of silence. |
| Stopping the app from Xcode, force-quitting it from the app switcher, or a crash | The process ends | Same automatic rejoin on next launch |
| iOS terminating the app for memory pressure in the background | Rare during an active call with audio, but possible | Same automatic rejoin |
| Network down for more than about 30 seconds | GPT-Live needs the network | The call falls back to on-device voice; messages to the agent still need the network and show as not sent |
| Signing out or switching workspace | The call's credentials change | The call ends |
| "Hot reload" | Swift apps have no in-process hot reload; changing code means installing a new build | See the first row |

### Starting a call cold, and how a call ends

A call reads the agent's DM history itself, without the inbox: when the
inbox has not filled the thread cache (an agent never messaged from this
device, or a cold start), the relay's conversation list is asked for the
thread and the id is remembered once found. The read tells three cases
apart: the thread's messages (everything in it counts as heard), no thread
yet (a known, empty conversation, so the first reply after the first spoken
request is read), and a failed read (unknown: nothing is read until a read
succeeds, so an old conversation is never read aloud). The first spoken
request creates the thread; later reads find and keep it.

Every path that ends a call names a cause (`VoiceCallEndCause`): hung up on
screen, a spoken goodbye, replaced by a new call, ended or reset by iOS,
signed out, microphone or speech denied, consent declined, audio failed,
stopped or lost its route, silence, on hold, unresponsive, a resume that
failed. Each cause has a reason the call screen shows under "Call ended", and
the call bar shows when the screen was closed, so "Call ended" never stands
alone. The ended bar shows only a call whose reason has not been seen (it
ended while its screen was closed, or a workspace switch ended it), never
over the sign-in screen, and a sign-out forgets the call. A thread the lookup
found is only the one clean 1:1 thread with the agent; a list that holds the
agent but cannot identify one such thread is a failed read, not "no thread".
Between reply checks the list's last answer holds for 30 seconds; a call that
starts asks again, and "no thread" is not recorded within a minute of a
request to the agent, whose first message creates the thread. The call logs a `stage=` line at each step (starting,
microphone, conversation, consent, callkit, gpt-live, on-device, listening,
cancelled, ended with its cause) to subsystem `com.agentworkforce.relayipad`,
category `voice-call`, so a report from the next build explains itself.

## (e) Security

- **No vendor keys in the app.** Tier 0 uses none. GPT-Live has no ephemeral
  client tokens: OpenAI's ephemeral `client_secrets` are documented only for the
  older Realtime API, and its GPT-Live WebRTC and WebSocket guides keep the
  project key on a trusted server. So the Agent Relay Cloud **voice broker** sits
  in session creation instead. It receives the phone's SDP offer (authenticated
  with the existing Cloud credential), creates the session with the project key,
  and returns only the SDP answer and session id. The key and the session
  configuration (model, prompt, delegation mode, `store: false`) never reach the
  phone, so a modified app cannot change the prompt or turn on storage. The broker
  enforces per-account minute quotas and concurrency (GPT-Live rate limits are
  concurrent sessions per project), sends a hashed per-person
  `OpenAI-Safety-Identifier` if the Live endpoint accepts that header (OpenAI
  documents it for Realtime; to be checked in M2), and can attach a
  server-side sideband (`/v1/live/sessions/{id}/attach`) to close sessions that
  exceed quota. Tier 1/2 vendors (M4) use the same broker.
- **The phone controls delegation.** Only the app turns a delegation into a DM,
  so GPT-Live cannot reach the agent or its tools except through the guarded
  path below.
- **No new secrets on the phone.** The call uses the person's existing
  messaging token from the Keychain, the same credential typed messages use.
- **Audio stays on device in Tier 0.** Only the transcript leaves the phone,
  as a DM, exactly like typing. Nothing is recorded or stored. On GPT-Live, call
  audio and transcripts go to OpenAI under the API data policy with session
  storage off (`store: false`); Agent Relay stores only what is sent as DMs.
- **No dangerous actions by voice alone.** Before a transcript is sent (Tier 0)
  or a delegation is forwarded (GPT-Live), the app checks it for destructive intent (delete, remove, deploy, release, merge,
  push, force, reset, drop, revoke, rotate keys, pay, …). A match holds the turn
  and shows the transcript with **Send to agent** and **Discard**; nothing is
  sent until the person taps. On GPT-Live the app also appends commentary asking
  the person to confirm on screen. The voice header also tells the agent that voice
  is never approval for destructive actions. This is a guard, not the security
  boundary: the agent's own permission settings and the broker's approval flow
  still decide what runs.
- **Misheard text is visible.** The live transcript and every sent turn are on
  screen and in the DM history.

## (f) Milestones and recommended default

**Main experience: GPT-Live** voice with the person's Relay agent as its
client-delegated backend, at $0.05 per call-minute. **Fallback and privacy
option: Tier 0** (Apple on-device speech, $0), which ships first as M1. Both are
behind `RelayVoiceCallEnabled`. Until the broker exists every call uses Tier 0.

| Milestone | Scope |
|---|---|
| **M1 (PR #708)** | The fallback path. Feature flag `RelayVoiceCallEnabled`: on in Debug, off in Release at first; on in every build since build 47 (October 29, 2026). **Call** button on an agent conversation. Tier 0 pipeline: voice-processing engine with AEC, energy VAD, on-device `SFSpeechRecognizer`, `AVSpeechSynthesizer` rendered through the engine, sentence splitting and speech cleanup, barge-in, push-to-talk or open mic, sound cues, voice header, destructive-intent confirmation, reply wait via push + poll, turn timeout (filler at 4 s, give up at 120 s), idle timeout (ends after 5 minutes of silence), interruption handling. Unit tests for the pure logic. |
| **M2: GPT-Live** | Cloud voice broker (`POST` SDP offer → GPT-Live session with client delegation, quota, usage records, optional sideband for quota enforcement). iOS libwebrtc client. Delegation handler reusing M1's DM transport, guard and speech cleanup (`session.delegation.created` → DM → `session.thinking.append` / `session.commentary.append`). Captions from transcript deltas. One-time consent screen; declining means no voice call. Idle close after 60 s with **Resume**. Fallback switching per the rules above. Usage reporting and latency telemetry. Tests: delegation request assembly, append trimming, fallback decisions. |
| M3 | Relaycast event socket for faster replies on both paths; `SpeechAnalyzer` on iOS 26 for the fallback; Live Activity. (Background audio shipped with M2; voice calls are on in every build since build 47.) |
| M4 | Opt-in Tier 1/2 voices for the fallback through the same broker. |

### Decisions

- **Decided (October 28, 2026):** GPT-Live uses the project's OpenAI
  credentials. Only the broker holds the key; it never appears in
  the app, the code, a PR or a message.
- **Decided (October 28, 2026):** while in development, each person gets
  100 GPT-Live minutes (6,000 seconds) per calendar month, enforced by the
  broker; there is no separate monthly spend cap for now.
- **Decided (October 28, 2026):** the voice broker is part of the Agent Relay
  Cloud API.
- **Decided (October 29, 2026):** voice calls are on in every build, Release
  included (`RelayVoiceCallEnabled` is `YES` for Debug and Release), so every
  TestFlight build has the Call button.
- **Still required before any App Store submission:** the privacy policy text
  and the App Store privacy answers for sending call audio to OpenAI (drafts
  below). The in-app consent card already uses the draft wording and asks
  before the first GPT-Live call. The TestFlight workflow refuses an
  `app-store` upload unless `voice_privacy_ready` is confirmed, so a
  review-eligible build can't go out before this is done.
- Any other paid vendor account (Deepgram, Cartesia, ElevenLabs, Groq, Google).

### Drafts for review (not final)

These drafts are for review before any production rollout. The
consent card is already in the Debug build so it can be tried out.

**Consent card, shown before the first GPT-Live call**

> **Use GPT-Live for voice calls?**
>
> GPT-Live from OpenAI makes calls sound natural: you can talk over it, and it
> answers right away while your agent works. During a GPT-Live call, your voice
> and the call transcript are sent to OpenAI. Your requests still go to your
> agent as messages. Agent Relay asks OpenAI not to store the call.
>
> If GPT-Live isn't available, the call falls back to on-device voice, which
> keeps your audio on this device.
>
> [Not now] [Use GPT-Live]

Choosing **Not now** ends the call before it starts, with: "Voice calls need
GPT-Live, so the call didn't start. Call again if you change your mind."

**Microphone permission string** (`NSMicrophoneUsageDescription`): "Agent Relay
uses the microphone during voice calls with your agents. With GPT-Live voice,
call audio is sent to OpenAI."

**Privacy policy paragraph**

> **Voice calls.** You can talk to your agents by voice in the Agent Relay
> mobile app. Calls use GPT-Live voice, which you agree to before your first
> call: the call's audio and transcript are sent to OpenAI to run the spoken conversation,
> under OpenAI's API data usage policies. We ask OpenAI not to store the call
> (session storage off) and do not record calls ourselves. We keep the length of
> each GPT-Live call to apply monthly limits. Your requests and your agent's
> replies are stored as messages, like typed messages.
> If GPT-Live isn't available, the call falls back to on-device voice: speech
> recognition and the agent's voice run on your device, and only the text of
> your requests is sent, as messages to your agent.

**App Store privacy answers (proposed)**

| Data type | Collected? | Linked to the person | Tracking | Purpose |
|---|---|---|---|---|
| Audio Data | Yes, only with GPT-Live voice: sent to OpenAI, which may keep API data for a limited time under its policies | Yes (the call belongs to a signed-in account) | No | App Functionality |
| Other User Content (call transcripts sent as messages) | Yes; already covered if messages are declared | Yes | No | App Functionality |
| Product Interaction / Other Usage Data (call duration for the monthly limit) | Yes | Yes | No | App Functionality |

Check before submitting: OpenAI's current API data retention terms for the
project (they decide whether audio counts as "collected"), and whether the
existing privacy answers already declare messages.

