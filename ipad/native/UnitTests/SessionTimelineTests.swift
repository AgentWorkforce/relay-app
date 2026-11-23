import XCTest
@testable import AgentRelayiPad

final class SessionTimelineTests: XCTestCase {
    private func message(_ id: String, at time: String?, text: String = "hi", sender: String = "agent") -> RelayMessage {
        var data: JSONObject = ["id": id, "text": text, "agent_name": sender]
        if let time { data["created_at"] = time }
        return RelayMessage(data, index: 0)
    }

    private func event(_ id: String, at time: String?, kind: String = "assistant", role: String = "assistant",
                       content: String, tool: String = "") -> SessionEvent {
        SessionEvent(key: .init(source: "codex", machineID: "m1", kind: kind, eventID: id),
                     ts: time.flatMap(SessionEventPage.date), kind: kind, actorRole: role, content: content, toolName: tool)
    }

    private func ids(_ items: [ChatTimelineItem]) -> [String] { items.map(\.id) }

    func testMergesBothSourcesInTimeOrder() {
        let messages = [message("m1", at: "2026-11-20T10:01:00Z"), message("m2", at: "2026-11-20T10:03:00Z")]
        let events = [event("e0", at: "2026-11-20T10:00:00Z", content: "first"),
                      event("e2", at: "2026-11-20T10:02:00Z", kind: "tool_use", content: "ls", tool: "Bash"),
                      event("e4", at: "2026-11-20T10:04:00Z", content: "last")]
        XCTAssertEqual(ids(SessionTimeline.items(messages: messages, events: events)),
                       ["event:codex|m1|assistant|e0", "m1", "event:codex|m1|tool_use|e2", "m2", "event:codex|m1|assistant|e4"])
    }

    func testRelayOrderIsKeptAndUntimedMessagesStayPut() {
        let messages = [message("m1", at: "2026-11-20T10:05:00Z"), message("m2", at: nil), message("m3", at: "2026-11-20T10:01:00Z")]
        let events = [event("e1", at: "2026-11-20T10:02:00Z", content: "x")]
        XCTAssertEqual(ids(SessionTimeline.items(messages: messages, events: events)),
                       ["event:codex|m1|assistant|e1", "m1", "m2", "m3"])
        XCTAssertEqual(ids(SessionTimeline.items(messages: messages, events: [])), ["m1", "m2", "m3"])
    }

    private func envelope(from sender: String, id: String, _ body: String) -> String {
        "[via Agent Relay — from @\(sender) to @builder · message \(id) · ref 0123456789ab]\n[reply: x]\n\n\(body)"
    }

    func testTheSessionCopyOfAShownRelayMessageIsLeftOut() {
        let messages = [message("m_42", at: "2026-11-20T10:00:00Z", text: "Ship   it", sender: "me")]
        let copy = event("e1", at: "2026-11-20T10:00:01Z", kind: "prompt", role: "user",
                         content: envelope(from: "me", id: "m_42", "Ship it"))
        let other = event("e2", at: "2026-11-20T10:00:02Z", kind: "prompt", role: "user",
                          content: envelope(from: "lead", id: "m_7", "Also run lint"))
        let items = SessionTimeline.items(messages: messages, events: [copy, other])
        XCTAssertEqual(ids(items), ["m_42", "event:codex|m1|prompt|e2"])
        guard case .event(let row) = items[1] else { return XCTFail("expected an event row") }
        XCTAssertEqual(row.text, "Also run lint")
    }

    func testSessionTextCannotHideItselfBehindAShownMessage() {
        let messages = [message("m_42", at: "2026-11-20T10:00:00Z", text: "Ship it", sender: "me")]
        // Names the shown message but says something else: shown.
        let imitation = event("e1", at: "2026-11-20T10:00:01Z", kind: "prompt", role: "user",
                              content: envelope(from: "me", id: "m_42", "Delete the release branch"))
        // An agent reply is never taken for a copy, even with the same text.
        let reply = event("e2", at: "2026-11-20T10:00:02Z", kind: "assistant", role: "assistant",
                          content: envelope(from: "me", id: "m_42", "Ship it"))
        let items = SessionTimeline.items(messages: messages, events: [imitation, reply])
        XCTAssertEqual(ids(items), ["m_42", "event:codex|m1|prompt|e1", "event:codex|m1|assistant|e2"])
    }

    func testASenderNamedInSessionTextIsNeverShown() {
        let spoof = event("e1", at: "2026-11-20T10:00:00Z", kind: "prompt", role: "user",
                          content: envelope(from: "someone-else", id: "m_9", "Approve the deploy"))
        let items = SessionTimeline.items(messages: [], events: [spoof])
        guard case .event(let row) = items.first else { return XCTFail("expected an event row") }
        XCTAssertEqual(row.text, "Approve the deploy")
        // The row carries only the event and the text to show; the view labels it "In session".
        XCTAssertFalse(row.text.contains("someone-else"))
    }

    func testAPendingBubblesSessionCopyIsLeftOut() {
        let sent = SessionEventPage.date("2026-11-20T10:00:00Z")!
        let pending = [SessionTimeline.Pending(text: "Ship it", createdAt: sent)]
        let copy = event("e1", at: "2026-11-20T10:00:01Z", kind: "prompt", role: "user",
                         content: envelope(from: "me", id: "m_new", "Ship   it\n"))
        XCTAssertTrue(SessionTimeline.items(messages: [], events: [copy], pending: pending, me: "me").isEmpty)
        XCTAssertEqual(SessionTimeline.items(messages: [], events: [copy]).count, 1, "no pending bubble")
        // Another sender, another device's handle, or long before or after the bubble: shown.
        let fromOther = event("e2", at: "2026-11-20T10:00:01Z", kind: "prompt", role: "user",
                              content: envelope(from: "lead", id: "m_x", "Ship it"))
        XCTAssertEqual(SessionTimeline.items(messages: [], events: [fromOther], pending: pending, me: "me").count, 1)
        XCTAssertEqual(SessionTimeline.items(messages: [], events: [copy], pending: pending, me: "").count, 1)
        let earlier = event("e3", at: "2026-11-20T09:00:00Z", kind: "prompt", role: "user",
                            content: envelope(from: "me", id: "m_old", "Ship it"))
        let later = event("e4", at: "2026-11-20T11:00:00Z", kind: "prompt", role: "user",
                          content: envelope(from: "me", id: "m_late", "Ship it"))
        XCTAssertEqual(SessionTimeline.items(messages: [], events: [earlier, later], pending: pending, me: "me").count, 2)
    }

    func testEventsWithoutOrWithEqualTimes() {
        let messages = [message("m1", at: "2026-11-20T10:00:00Z")]
        let untimed = event("e0", at: nil, content: "no time")
        let same = event("e1", at: "2026-11-20T10:00:00Z", content: "same time")
        // An untimed event comes first; at an equal time the relay message leads.
        XCTAssertEqual(ids(SessionTimeline.items(messages: messages, events: [untimed, same])),
                       ["event:codex|m1|assistant|e0", "m1", "event:codex|m1|assistant|e1"])
    }

    func testPreambleIsStrippedAndPreambleOnlyPromptsHidden() {
        let prompt = event("e1", at: "2026-11-20T10:00:00Z", kind: "prompt", role: "user",
                           content: "<environment_context>\n<cwd>/x</cwd>\n</environment_context>\nFix the build")
        let only = event("e2", at: "2026-11-20T10:00:01Z", kind: "prompt", role: "user",
                         content: "<system_instruction>setup</system_instruction>")
        let items = SessionTimeline.items(messages: [], events: [prompt, only])
        XCTAssertEqual(items.count, 1)
        guard case .event(let row) = items[0] else { return XCTFail("expected an event row") }
        XCTAssertEqual(row.text, "Fix the build")
    }

    private func rows(_ items: [ChatTimelineItem]) -> [SessionEventRow] {
        items.compactMap { if case .event(let row) = $0 { row } else { nil } }
    }

    func testThinkingIsNotShown() {
        let events = [event("e1", at: "2026-11-20T10:00:00Z", kind: "reasoning", content: "Weighing two fixes"),
                      SessionEvent(key: .init(source: "codex", machineID: "m1", kind: "x", eventID: "e2"),
                                   ts: SessionEventPage.date("2026-11-20T10:00:01Z"), kind: "x", type: "thinking",
                                   actorRole: "assistant", content: "Still weighing"),
                      event("e3", at: "2026-11-20T10:00:02Z", content: "Going with the smaller fix.")]
        let items = SessionTimeline.items(messages: [], events: events)
        XCTAssertEqual(ids(items), ["event:codex|m1|assistant|e3"])
        // Blank content shows the tool's name, or the event is left out.
        let blank = [event("t1", at: "2026-11-20T10:00:03Z", kind: "tool_use", content: " ", tool: "Read"),
                     event("t2", at: "2026-11-20T10:00:04Z", kind: "future_kind", role: "", content: "\n")]
        XCTAssertEqual(rows(SessionTimeline.items(messages: [], events: blank)).map(\.text), ["Read"])
    }

    func testAnOutgoingSendIsShownOnceAndMarked() {
        let send = "agent-relay-probe send --socket s --to @scout --body 'Release notes are ready.'"
        let events = [event("e1", at: "2026-11-20T10:00:00Z", kind: "tool_use", content: send, tool: "Bash"),
                      // Cloud's text copy of the call, and the call recorded again.
                      event("e2", at: "2026-11-20T10:00:01Z", content: send),
                      event("e3", at: "2026-11-20T10:00:02Z", kind: "tool_call", content: send, tool: "Bash"),
                      // Its output is not a send.
                      event("e4", at: "2026-11-20T10:00:03Z", kind: "tool_result", role: "tool_result", content: send)]
        let shown = rows(SessionTimeline.items(messages: [], events: events))
        XCTAssertEqual(shown.map(\.id), ["event:codex|m1|tool_use|e1", "event:codex|m1|tool_result|e4"])
        XCTAssertEqual(shown[0].text, "Release notes are ready.")
        XCTAssertEqual(shown[0].mark, .to("scout"))
        XCTAssertNil(shown[1].mark)
        // A relay tool call with no recipient is still a send.
        let post = event("e5", at: "2026-11-20T10:00:04Z", kind: "tool_use",
                         content: #"send_relay_message {"text": "Done."}"#, tool: "send_relay_message")
        XCTAssertEqual(rows(SessionTimeline.items(messages: [], events: [post])).first?.mark, .to(nil))
        // The agent's relay message the chat already holds stays; the session's
        // copy of its send to this device's handle goes.
        let reply = message("m_9", at: "2026-11-20T10:00:05Z", text: "Release notes are ready.", sender: "builder")
        let toMe = event("e6", at: "2026-11-20T10:00:04Z", kind: "tool_use",
                         content: "agent-relay-probe send --to @pilot --body 'Release notes are ready.'", tool: "Bash")
        XCTAssertEqual(ids(SessionTimeline.items(messages: [reply], events: [toMe], me: "pilot")), ["m_9"])
        // Direct messages alone come through unchanged.
        XCTAssertEqual(ids(SessionTimeline.items(messages: [reply], events: [], me: "pilot")), ["m_9"])
    }

    func testTheSameTextSentToSomeoneElseIsShown() {
        // The chat holds the agent's reply to this device; its send of the
        // same text to scout is another message.
        let reply = message("m_9", at: "2026-11-20T10:00:05Z", text: "Release notes are ready.", sender: "builder")
        let toScout = event("e1", at: "2026-11-20T10:00:00Z", kind: "tool_use",
                            content: "agent-relay-probe send --to @scout --body 'Release notes are ready.'", tool: "Bash")
        XCTAssertEqual(ids(SessionTimeline.items(messages: [reply], events: [toScout], me: "pilot")),
                       ["event:codex|m1|tool_use|e1", "m_9"])
        // A repeated send to another recipient is shown; to the same one, once.
        let toCrew = event("e2", at: "2026-11-20T10:00:01Z", kind: "tool_use",
                           content: #"post_relay_channel {"channel": "crew", "text": "Release notes are ready."}"#,
                           tool: "post_relay_channel")
        let toScoutAgain = event("e3", at: "2026-11-20T10:00:02Z", kind: "tool_use",
                                 content: "agent-relay-probe send --to scout --body 'Release notes are ready.'", tool: "Bash")
        let shown = rows(SessionTimeline.items(messages: [], events: [toScout, toCrew, toScoutAgain]))
        XCTAssertEqual(shown.map(\.mark), [.to("scout"), .to("#crew")])
        // The same send long after is a new message.
        let later = event("e4", at: "2026-11-20T10:10:01Z", kind: "tool_use",
                          content: "agent-relay-probe send --to scout --body 'Release notes are ready.'", tool: "Bash")
        XCTAssertEqual(rows(SessionTimeline.items(messages: [], events: [toScout, later])).count, 2)
        XCTAssertEqual(SessionTimeline.relayRepeatWindow, 10 * 60)
    }

    func testARepeatIsWithinTheWindowOrUntimed() {
        func key(_ to: String?) -> SessionTimeline.RelayKey { SessionTimeline.relayKey(incoming: false, counterpart: to, "Hi") }
        var seen = SessionTimeline.RelaySeen()
        let at = SessionEventPage.date("2026-11-20T10:00:00Z")!
        seen.record(key("scout"), at: at)
        XCTAssertTrue(seen.isRepeat(key("scout"), at: at.addingTimeInterval(SessionTimeline.relayRepeatWindow)))
        XCTAssertFalse(seen.isRepeat(key("scout"), at: at.addingTimeInterval(SessionTimeline.relayRepeatWindow + 1)))
        // With no time on either side the key alone decides.
        XCTAssertTrue(seen.isRepeat(key("scout"), at: nil))
        var untimed = SessionTimeline.RelaySeen()
        untimed.record(key("scout"), at: nil)
        XCTAssertTrue(untimed.isRepeat(key("scout"), at: at.addingTimeInterval(86_400)))
        XCTAssertFalse(seen.isRepeat(key("#crew"), at: at))
        XCTAssertFalse(seen.isRepeat(SessionTimeline.relayKey(incoming: true, counterpart: "scout", "Hi"), at: at))
        // An unnamed side matches any; a named one only itself.
        XCTAssertTrue(seen.isRepeat(key(nil), at: at))
        var unnamed = SessionTimeline.RelaySeen()
        unnamed.record(key(""), at: at)
        XCTAssertTrue(unnamed.isRepeat(key("pilot"), at: at))
        // The other side is a bare handle.
        XCTAssertEqual(SessionTimeline.relayKey(incoming: true, counterpart: "@pilot", " Ship \n it "),
                       SessionTimeline.RelayKey(incoming: true, counterpart: "pilot", text: "Ship it"))
        XCTAssertEqual(key("pilot@harbor").counterpart, "pilot")
        XCTAssertEqual(key("@pilot@harbor").counterpart, "pilot")
        XCTAssertEqual(key("#crew").counterpart, "#crew")
    }

    func testAHeldReplyHidesItsSendByAddressOrUnnamedSide() {
        let reply = message("m_9", at: "2026-11-20T10:00:05Z", text: "Done.", sender: "builder")
        func shown(_ content: String, me: String, tool: String = "Bash", at time: String? = "2026-11-20T10:00:04Z",
                   held: RelayMessage? = nil) -> [String] {
            let send = event("e1", at: time, kind: "tool_use", content: content, tool: tool)
            return ids(SessionTimeline.items(messages: [held ?? reply], events: [send], me: me))
        }
        // A send by address names the same handle as this device's.
        XCTAssertEqual(shown("agent-relay-probe send --address pilot@harbor --body 'Done.'", me: "pilot"), ["m_9"])
        // With no handle for this device, a send to someone still matches the held reply.
        XCTAssertEqual(shown("agent-relay-probe send --to @pilot --body 'Done.'", me: ""), ["m_9"])
        // A relay tool call that names no recipient matches it too.
        XCTAssertEqual(shown(#"send_relay_message {"text": "Done."}"#, me: "pilot", tool: "send_relay_message"), ["m_9"])
        // With no time on the held reply, or on the send, the key alone decides.
        let untimedReply = message("m_9", at: nil, text: "Done.", sender: "builder")
        XCTAssertEqual(shown("agent-relay-probe send --to @pilot --body 'Done.'", me: "pilot", held: untimedReply), ["m_9"])
        XCTAssertEqual(shown("agent-relay-probe send --to @pilot --body 'Done.'", me: "pilot", at: nil), ["m_9"])
        // A send to someone else by address is still shown.
        XCTAssertEqual(shown("agent-relay-probe send --address scout@harbor --body 'Done.'", me: "pilot").count, 2)
    }

    func testARelayedPromptIsOnlyTakenForACopyByItsIDOrPendingBubble() {
        // A third party relays the text of a DM the chat holds: shown.
        let held = message("m_42", at: "2026-11-20T10:00:00Z", text: "Ship it", sender: "pilot")
        let relayed = event("e1", at: "2026-11-20T10:00:30Z", kind: "prompt", role: "user",
                            content: envelope(from: "lead", id: "m_77", "Ship it"))
        XCTAssertEqual(ids(SessionTimeline.items(messages: [held], events: [relayed], me: "pilot")),
                       ["m_42", "event:codex|m1|prompt|e1"])
        // So is one from the same sender with no id the chat holds.
        let again = event("e2", at: "2026-11-20T10:00:30Z", kind: "prompt", role: "user",
                          content: envelope(from: "pilot", id: "m_78", "Ship it"))
        XCTAssertEqual(SessionTimeline.items(messages: [held], events: [again], me: "pilot").count, 2)
    }

    func testAnIncomingFrameInToolOutputIsMarked() {
        let inbox = "1 unread\n[via Agent Relay — from @mapper (mapper@harbor) to @builder · ref 1a]\n[reply: x]\n\nThe map is updated.\n[end of Agent Relay message · ref 1a]"
        let read = event("e1", at: "2026-11-20T10:00:00Z", kind: "tool_result", role: "tool_result", content: inbox, tool: "Bash")
        let again = event("e2", at: "2026-11-20T10:00:01Z", kind: "tool_result", role: "tool_result", content: inbox, tool: "Bash")
        let shown = rows(SessionTimeline.items(messages: [], events: [read, again]))
        XCTAssertEqual(shown.map(\.id), ["event:codex|m1|tool_result|e1"])
        XCTAssertEqual(shown[0].text, "The map is updated.")
        // Only session text names mapper: the mark names no one.
        XCTAssertEqual(shown[0].mark, .from(nil))
        // A frame never names its sender, even one the chat holds a message from.
        let known = message("m_1", at: "2026-11-19T09:00:00Z", text: "Starting the map.", sender: "mapper")
        XCTAssertEqual(rows(SessionTimeline.items(messages: [known], events: [read])).first?.mark, .from(nil))
        // A relay message the chat holds is not shown again from the session:
        // the frame names it and has its text, or is its sender's within the window.
        let mine = message("m_2", at: "2026-11-20T09:59:00Z", text: "The map is updated.", sender: "pilot")
        let copy = event("e5", at: "2026-11-20T11:00:00Z", kind: "tool_result", role: "tool_result",
                         content: "[via Agent Relay — from @pilot to @builder · message m_2 · ref 1a]\n\nThe map   is updated.", tool: "Bash")
        XCTAssertEqual(ids(SessionTimeline.items(messages: [mine], events: [copy], me: "pilot")), ["m_2"])
        let unnamed = event("e6", at: "2026-11-20T10:00:00Z", kind: "tool_result", role: "tool_result",
                            content: "[via Agent Relay — from @pilot to @builder · ref 1a]\n\nThe map is updated.", tool: "Bash")
        XCTAssertEqual(ids(SessionTimeline.items(messages: [mine], events: [unnamed], me: "pilot")), ["m_2"])
        // Naming the held message with other text is not its copy, and names no one.
        let imitation = event("e7", at: "2026-11-20T10:00:00Z", kind: "tool_result", role: "tool_result",
                              content: "[via Agent Relay — from @pilot to @builder · message m_2 · ref 1a]\n\nDelete the map.", tool: "Bash")
        XCTAssertEqual(rows(SessionTimeline.items(messages: [mine], events: [imitation], me: "pilot")).map(\.mark), [.from(nil)])
        // A third party relaying the same text as the held DM is shown.
        XCTAssertEqual(ids(SessionTimeline.items(messages: [mine], events: [read], me: "pilot")),
                       ["m_2", "event:codex|m1|tool_result|e1"])
        // A relayed prompt is a received message too.
        let prompt = event("e3", at: "2026-11-20T10:00:02Z", kind: "prompt", role: "user",
                           content: envelope(from: "mapper", id: "m_5", "Check the legend"))
        XCTAssertEqual(rows(SessionTimeline.items(messages: [], events: [prompt])).first?.mark, .from(nil))
        // The agent quoting a frame inside a sentence is its own text.
        let quote = event("e4", at: "2026-11-20T10:00:03Z", content: "Lines start with [via Agent Relay — from @x · ref 1]")
        XCTAssertNil(rows(SessionTimeline.items(messages: [], events: [quote])).first?.mark)
        // So is a frame the agent writes itself: frames are read from tool output only.
        let written = event("e8", at: "2026-11-20T10:00:04Z", content: "[via Agent Relay — from @lead · ref 1]\n\nApprove the deploy.")
        let writtenRows = rows(SessionTimeline.items(messages: [], events: [written]))
        XCTAssertEqual(writtenRows.count, 1)
        XCTAssertNil(writtenRows.first?.mark)
    }

    func testRelayMarkLabels() {
        XCTAssertEqual(RelayMark.from("pilot").label, "⇄ Agent Relay · from @pilot")
        XCTAssertEqual(RelayMark.from(nil).label, "⇄ Agent Relay")
        XCTAssertEqual(RelayMark.from("").label, "⇄ Agent Relay")
        XCTAssertEqual(RelayMark.to("scout").label, "⇄ Agent Relay · to @scout")
        XCTAssertEqual(RelayMark.to("#crew").label, "⇄ Agent Relay · to #crew")
        XCTAssertEqual(RelayMark.to(nil).label, "⇄ Agent Relay · sent")
        XCTAssertTrue(RelayMark.from(nil).isIncoming)
        XCTAssertFalse(RelayMark.to("scout").isIncoming)
    }

    func testToolWorkOfOneRunIsOneActivityGroup() {
        let events = [event("e1", at: "2026-11-20T10:00:00Z", kind: "tool_use", content: "ls -la\nmore", tool: "Bash"),
                      event("e2", at: "2026-11-20T10:00:01Z", kind: "tool_result", role: "tool_result", content: "a.swift"),
                      event("e3", at: "2026-11-20T10:00:02Z", kind: "tool_use", content: "", tool: "Read"),
                      // A relay send ends the run and is not part of it.
                      event("e4", at: "2026-11-20T10:00:03Z", kind: "tool_use",
                            content: "agent-relay-probe send --to scout --body 'Halfway.'", tool: "Bash"),
                      event("e5", at: "2026-11-20T10:00:04Z", kind: "tool_use", content: "swift test", tool: "Bash"),
                      // An activity note ends a run too.
                      event("e6", at: "2026-11-20T10:00:05Z", kind: "future_kind", role: "", content: "Checkpoint saved"),
                      event("e7", at: "2026-11-20T10:00:06Z", kind: "tool_use", content: "git diff", tool: "Bash"),
                      event("e8", at: "2026-11-20T10:00:07Z", content: "Done.")]
        let messages = [message("m1", at: "2026-11-20T10:00:04.5Z", text: "Any news?", sender: "pilot")]
        let grouped = SessionTimeline.grouped(SessionTimeline.items(messages: messages, events: events))
        XCTAssertEqual(ids(grouped), ["activity:codex|m1|tool_use|e3", "event:codex|m1|tool_use|e4",
                                      "activity:codex|m1|tool_use|e5", "m1", "event:codex|m1|future_kind|e6",
                                      "activity:codex|m1|tool_use|e7", "event:codex|m1|assistant|e8"])
        guard case .activity(let first) = grouped[0] else { return XCTFail("expected a group") }
        XCTAssertEqual(first.rows.count, 3)
        XCTAssertEqual(ActivityGroup.line(for: first.rows[0]).title, "Bash")
        XCTAssertEqual(ActivityGroup.line(for: first.rows[0]).target, "ls -la")
        XCTAssertEqual(ActivityGroup.line(for: first.rows[1]).title, "Result")
        XCTAssertEqual(ActivityGroup.line(for: first.rows[2]).target, "")
        // An older call joining the run keeps the group's id.
        let longer = SessionTimeline.grouped(SessionTimeline.items(messages: [], events: Array(events[1...2])))
        XCTAssertEqual(longer.first?.id, first.id)
    }

    func testAGroupStaysFoundAndOpenWhenOlderOrNewerWorkJoinsIt() {
        let older = event("e0", at: "2026-11-20T09:59:59Z", kind: "tool_use", content: "pwd", tool: "Bash")
        let calls = [event("e1", at: "2026-11-20T10:00:00Z", kind: "tool_use", content: "ls", tool: "Bash"),
                     event("e2", at: "2026-11-20T10:00:01Z", kind: "tool_result", role: "tool_result", content: "a.swift")]
        let newer = event("e3", at: "2026-11-20T10:00:02Z", kind: "tool_use", content: "swift test", tool: "Bash")
        func group(_ events: [SessionEvent]) -> ActivityGroup? {
            if case .activity(let group) = SessionTimeline.grouped(SessionTimeline.items(messages: [], events: events)).first {
                return group
            }
            return nil
        }
        guard let before = group(calls), let afterOlder = group([older] + calls),
              let afterNewer = group(calls + [newer]) else { return XCTFail("expected groups") }
        // An older page never moves the group's id, so the scroll back to
        // the row that was first before the page finds it.
        XCTAssertEqual(before.id, "activity:codex|m1|tool_result|e2")
        XCTAssertEqual(afterOlder.id, before.id)
        // A newer row joining the newest run moves it: a new view of the group.
        XCTAssertNotEqual(afterNewer.id, before.id)
        // Opened before either joined, the group stays open after both; so does an opened line.
        var expansion = ActivityExpansion()
        XCTAssertFalse(expansion.isExpanded(before))
        expansion.setExpanded(true, before)
        expansion.setOpened(before.rows[1], true)
        XCTAssertTrue(expansion.isExpanded(afterOlder))
        XCTAssertTrue(expansion.isExpanded(afterNewer))
        XCTAssertTrue(expansion.isOpened(afterOlder.rows[2]))
        // Closed after older work joined, no row keeps it open.
        expansion.setExpanded(false, afterOlder)
        XCTAssertFalse(expansion.isExpanded(before))
        XCTAssertFalse(expansion.isExpanded(afterNewer))
        expansion.setOpened(before.rows[1], false)
        XCTAssertFalse(expansion.isOpened(afterOlder.rows[2]))
    }

    func testOpeningALineIsTheSameOnEveryCopy() {
        let row = SessionEventRow(event: event("e1", at: "2026-11-20T10:00:00Z", kind: "tool_use", content: "ls", tool: "Bash"),
                                  text: "ls")
        // Two live views of one group (two scenes) and the kept copy: one
        // view opens the line, then the other, from what each shows.
        var kept = ActivityExpansion(), first = ActivityExpansion(), second = ActivityExpansion()
        let open = !first.isOpened(row)
        first.setOpened(row, open)
        kept.setOpened(row, open)
        // The second view was made before and still shows it closed: it
        // asks for open, and setting it again changes nothing.
        let alsoOpen = !second.isOpened(row)
        second.setOpened(row, alsoOpen)
        kept.setOpened(row, alsoOpen)
        XCTAssertTrue(kept.isOpened(row))
        XCTAssertTrue(second.isOpened(row))
        // Closing is the same on every copy, however often it is applied.
        kept.setOpened(row, false)
        kept.setOpened(row, false)
        first.setOpened(row, false)
        XCTAssertFalse(kept.isOpened(row))
        XCTAssertEqual(kept, first)
    }

    func testTheTurnIsOpenUntilTheAgentReplies() {
        func open(_ events: [SessionEvent], messages: [RelayMessage] = [], me: String = "") -> Bool {
            SessionTurn.isOpen(SessionTimeline.items(messages: messages, events: events, me: me), events: events, me: me)
        }
        let prompt = event("e1", at: "2026-11-20T10:00:00Z", kind: "prompt", role: "user", content: "Fix the build")
        let tool = event("e2", at: "2026-11-20T10:00:01Z", kind: "tool_use", content: "make", tool: "Bash")
        let reply = event("e3", at: "2026-11-20T10:00:02Z", content: "Fixed.")
        let thinking = event("e4", at: "2026-11-20T10:00:03Z", kind: "reasoning", content: "Should I also tag it?")
        let note = event("e5", at: "2026-11-20T10:00:04Z", kind: "future_kind", role: "", content: "Checkpoint saved")
        XCTAssertTrue(open([prompt]))
        XCTAssertTrue(open([prompt, tool]))
        XCTAssertFalse(open([prompt, tool, reply]))
        // Hidden thinking after the reply still says it is working; a note says nothing.
        XCTAssertTrue(open([prompt, tool, reply, thinking]))
        XCTAssertFalse(open([prompt, tool, reply, note]))
        XCTAssertFalse(open([]))
        // A relay send closes the turn; a received relay message opens it.
        let send = event("e6", at: "2026-11-20T10:00:05Z", kind: "tool_use",
                         content: "agent-relay-probe send --to scout --body 'Fixed.'", tool: "Bash")
        XCTAssertFalse(open([prompt, send]))
        let inbox = event("e7", at: "2026-11-20T10:00:06Z", kind: "tool_result", role: "tool_result",
                          content: "[via Agent Relay — from @scout · ref 1]\n\nAlso tag it.")
        XCTAssertTrue(open([prompt, send, inbox]))
        // Direct messages: one to the agent opens it, the agent's closes it.
        let asked = message("m1", at: "2026-11-20T10:01:00Z", text: "Tag it too", sender: "pilot")
        let answered = message("m2", at: "2026-11-20T10:02:00Z", text: "Tagged.", sender: "builder")
        XCTAssertTrue(open([reply], messages: [asked], me: "pilot"))
        XCTAssertFalse(open([reply], messages: [asked, answered], me: "pilot"))
    }

    func testWorkingIsBoundedByAgeAndPresence() {
        let now = SessionEventPage.date("2026-11-20T10:00:00Z")!
        XCTAssertTrue(SessionTurn.showsWorking(open: true, newestEventAt: now.addingTimeInterval(-9 * 60), offline: false, now: now))
        XCTAssertFalse(SessionTurn.showsWorking(open: true, newestEventAt: now.addingTimeInterval(-SessionTurn.window), offline: false, now: now))
        XCTAssertFalse(SessionTurn.showsWorking(open: false, newestEventAt: now, offline: false, now: now))
        XCTAssertFalse(SessionTurn.showsWorking(open: true, newestEventAt: now, offline: true, now: now))
        XCTAssertFalse(SessionTurn.showsWorking(open: true, newestEventAt: nil, offline: false, now: now))
        XCTAssertTrue(SessionTurn.isOffline(status: "offline", lastSeen: nil, now: now))
        XCTAssertTrue(SessionTurn.isOffline(status: "active", lastSeen: now.addingTimeInterval(-6 * 60), now: now))
        XCTAssertFalse(SessionTurn.isOffline(status: "idle", lastSeen: now.addingTimeInterval(-60), now: now))
        XCTAssertFalse(SessionTurn.isOffline(status: "", lastSeen: nil, now: now))
    }

    func testPlaceholder() {
        // Loading until relay history is checked and the session settled.
        XCTAssertEqual(SessionTimeline.placeholder(hasRows: false, historyChecked: false, sessionApplies: false, sessionSettled: false), .loading)
        XCTAssertEqual(SessionTimeline.placeholder(hasRows: true, historyChecked: true, sessionApplies: true, sessionSettled: false), .loading)
        XCTAssertEqual(SessionTimeline.placeholder(hasRows: false, historyChecked: true, sessionApplies: true, sessionSettled: true), .empty)
        XCTAssertEqual(SessionTimeline.placeholder(hasRows: false, historyChecked: true, sessionApplies: false, sessionSettled: false), .empty)
        XCTAssertEqual(SessionTimeline.placeholder(hasRows: true, historyChecked: true, sessionApplies: true, sessionSettled: true), .none)
    }

    func testPreviewCutsLongContent() {
        XCTAssertEqual(SessionTimeline.preview("short").text, "short")
        XCTAssertFalse(SessionTimeline.preview("short").isCut)
        let long = SessionTimeline.preview(String(repeating: "x", count: 700))
        XCTAssertTrue(long.isCut)
        XCTAssertEqual(long.text.count, 601)
        let tall = SessionTimeline.preview((1...20).map(String.init).joined(separator: "\n"))
        XCTAssertTrue(tall.isCut)
        XCTAssertEqual(tall.text, (1...12).map(String.init).joined(separator: "\n") + "…")
    }

    func testCollapsedRowKeepsThreeLinesAndALimit() {
        XCTAssertEqual(SessionTimeline.collapsed("a\nb", characters: 240), "a\nb")
        XCTAssertEqual(SessionTimeline.collapsed("1\n2\n3\n4\n5", characters: 240), "1\n2\n3…")
        XCTAssertEqual(SessionTimeline.collapsed(String(repeating: "x", count: 300), characters: 240),
                       String(repeating: "x", count: 240) + "…")
    }

    func testFilesSummary() {
        XCTAssertEqual(SessionTimeline.filesSummary([]), "")
        XCTAssertEqual(SessionTimeline.filesSummary(["a/b.swift", "c.swift"]), "b.swift, c.swift")
        XCTAssertEqual(SessionTimeline.filesSummary(["a", "b", "c", "d", "e"]), "a, b, c +2")
    }
}
