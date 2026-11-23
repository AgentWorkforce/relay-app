import SwiftUI

/// An agent's question as a card: its questions, numbered option buttons,
/// a text field where free text is allowed, the time left and where the
/// answer stands. Nothing is sent without the person's tap, and one tap
/// sends one answer line through `send`.
struct QuestionCardView: View {
    let question: RelayQuestion
    let expiresAt: Date?
    /// The card's state at a given time.
    let state: (Date) -> RelayQuestionState
    let accent: Color
    /// Sends one "answer …" line to the agent this chat is with.
    let send: (String) -> Void

    @State private var picked: [Set<Int>] = []
    @State private var texts: [String] = []
    @State private var problem: String?
    /// Set by a tap until the card's state moves on, so one tap sends once.
    @State private var submitted = false

    /// A start that does not move between renders.
    private static let scheduleStart = Date(timeIntervalSinceReferenceDate: 0)

    /// One single-choice question: a tap on an option is the answer.
    private var answersOnTap: Bool {
        question.items.count == 1 && question.items[0].selection == .single
    }

    var body: some View {
        TimelineView(.periodic(from: Self.scheduleStart, by: 15)) { context in
            let current = state(context.date)
            card(current, now: context.date)
                .onChange(of: current) { _, next in
                    if next.answerable { submitted = false }
                }
        }
        .onAppear {
            if picked.count != question.items.count {
                picked = Array(repeating: [], count: question.items.count)
                texts = Array(repeating: "", count: question.items.count)
            }
        }
    }

    private func card(_ current: RelayQuestionState, now: Date) -> some View {
        let canSend = current.answerable && !submitted
        return VStack(alignment: .leading, spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Label("Question from @\(question.agent)", systemImage: "questionmark.bubble")
                    .font(.subheadline.weight(.semibold))
                if let workingOn = question.workingOn, !workingOn.isEmpty {
                    Text("Working on: \(workingOn)").font(.caption).foregroundStyle(.secondary)
                }
            }
            .accessibilityElement(children: .combine)
            ForEach(Array(question.items.enumerated()), id: \.offset) { index, item in
                itemView(item, index: index, canSend: canSend)
            }
            if !answersOnTap || hasAnyText {
                Button {
                    submit(responses())
                } label: {
                    Text("Send answer").frame(maxWidth: .infinity)
                }
                .buttonStyle(.borderedProminent).tint(accent)
                .disabled(!canSend || !complete)
                .accessibilityIdentifier("Send question answer")
            }
            if let problem, canSend {
                Text(problem).font(.caption).foregroundStyle(.red)
                    .fixedSize(horizontal: false, vertical: true)
            }
            Text(Self.status(current, expiresAt: expiresAt, minutes: question.expiresInMinutes, now: now))
                .font(.caption).foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
                .accessibilityIdentifier("Question state")
        }
        .padding(14)
        .frame(maxWidth: 520, alignment: .leading)
        .background(Color(.secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).strokeBorder(accent.opacity(0.35)))
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Question from \(question.agent)")
        .accessibilityIdentifier("Question card")
    }

    @ViewBuilder
    private func itemView(_ item: RelayQuestion.Item, index: Int, canSend: Bool) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            VStack(alignment: .leading, spacing: 2) {
                if question.items.count > 1 {
                    Text("Question \(index + 1) of \(question.items.count)")
                        .font(.caption2).foregroundStyle(.secondary)
                }
                if let header = item.header {
                    Text(header).font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                }
                Text(item.body).font(.body).fixedSize(horizontal: false, vertical: true)
            }
            .accessibilityElement(children: .combine)
            if item.selection == .multiple {
                Text("Choose one or more").font(.caption).foregroundStyle(.secondary)
            }
            ForEach(item.options, id: \.number) { option in
                optionButton(option, item: item, index: index, canSend: canSend)
            }
            if item.allowsFreeText {
                TextField(item.options.isEmpty ? "Your answer" : "Or answer in your own words",
                          text: textBinding(index), axis: .vertical)
                    .textFieldStyle(.roundedBorder)
                    .disabled(!canSend)
                    .accessibilityLabel("Answer in your own words")
                    .accessibilityIdentifier("Question free text \(index + 1)")
                if question.items.count > 1, text(index).contains(";") {
                    Text("A “;” is sent as “,” so it does not split the answers.")
                        .font(.caption2).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
    }

    private func optionButton(_ option: RelayQuestion.Option, item: RelayQuestion.Item, index: Int,
                              canSend: Bool) -> some View {
        let selected = picked.indices.contains(index) && picked[index].contains(option.number)
        return Button {
            choose(option.number, item: item, index: index)
        } label: {
            HStack(alignment: .firstTextBaseline, spacing: 8) {
                Image(systemName: item.selection == .multiple
                      ? (selected ? "checkmark.square.fill" : "square")
                      : (selected ? "largecircle.fill.circle" : "circle"))
                    .foregroundStyle(accent)
                VStack(alignment: .leading, spacing: 2) {
                    Text("\(option.number). \(option.label)").font(.body)
                        .fixedSize(horizontal: false, vertical: true)
                    if let description = option.description {
                        Text(description).font(.caption).foregroundStyle(.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
                Spacer(minLength: 0)
            }
            .padding(.vertical, 8).padding(.horizontal, 10)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(selected ? accent.opacity(0.12) : Color(.tertiarySystemGroupedBackground),
                        in: RoundedRectangle(cornerRadius: 10))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .disabled(!canSend)
        .accessibilityLabel("Option \(option.number): \(option.label)" + (option.description.map { ", \($0)" } ?? ""))
        .accessibilityHint(answersOnTap ? "Sends this answer" : "")
        .accessibilityAddTraits(selected ? .isSelected : [])
    }

    // MARK: Answers

    private func text(_ index: Int) -> String { texts.indices.contains(index) ? texts[index] : "" }

    private var hasAnyText: Bool {
        texts.contains { !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
    }

    private func textBinding(_ index: Int) -> Binding<String> {
        Binding(get: { text(index) }, set: { value in
            guard texts.indices.contains(index) else { return }
            texts[index] = value
            // Text and numbers are two kinds of answer; the latest one counts.
            if !value.isEmpty, picked.indices.contains(index) { picked[index] = [] }
            problem = nil
        })
    }

    private func choose(_ number: Int, item: RelayQuestion.Item, index: Int) {
        guard picked.indices.contains(index) else { return }
        problem = nil
        if answersOnTap {
            submit([.choices([number])])
            return
        }
        if item.selection == .multiple {
            if picked[index].contains(number) { picked[index].remove(number) } else { picked[index].insert(number) }
        } else {
            picked[index] = [number]
        }
        texts[index] = ""
    }

    private func responses() -> [RelayQuestionAnswer.Response] {
        question.items.indices.map { index in
            let chosen = picked.indices.contains(index) ? picked[index] : []
            return chosen.isEmpty ? .text(text(index)) : .choices(chosen.sorted())
        }
    }

    private var complete: Bool {
        question.items.indices.allSatisfy { index in
            (picked.indices.contains(index) && !picked[index].isEmpty)
                || !text(index).trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        }
    }

    /// Builds the line and sends it, once per tap and only while the card
    /// can be answered.
    private func submit(_ responses: [RelayQuestionAnswer.Response]) {
        guard !submitted, state(Date()).answerable else { return }
        switch RelayQuestionAnswer.line(for: question, responses: responses) {
        case .success(let line):
            submitted = true
            send(line)
        case .failure(let failure):
            problem = Self.message(for: failure)
        }
    }

    static func message(for problem: RelayQuestionAnswer.Problem) -> String {
        switch problem {
        case .wrongCount: "Answer every question."
        case .unanswered(let number): "Answer question \(number)."
        case .invalidChoice(let number): "Choose one of the options for question \(number)."
        case .freeTextNotAllowed(let number): "Question \(number) takes one of its options."
        case .looksLikeChoice(let number): "Tap an option for question \(number) instead of typing its number."
        case .tooLong: "The answer is too long to send."
        }
    }

    /// The card's last line: where the answer stands, or the time left.
    static func status(_ state: RelayQuestionState, expiresAt: Date?, minutes: Int, now: Date) -> String {
        switch state {
        case .open:
            guard let expiresAt else { return "Expires in \(minutes) min." }
            let left = max(1, Int((expiresAt.timeIntervalSince(now) / 60).rounded(.up)))
            return "Expires in \(left) min."
        case .sending: return "Sending your answer…"
        case .sendFailed: return "Your answer could not be confirmed. Tap its message below to try again."
        case .sent: return "Answer sent."
        case .delivered: return "Answer delivered."
        case .queued: return "Answer queued: it reaches the session at its next turn."
        case .notDelivered(let reason):
            return reason.isEmpty ? "The answer was not delivered. You can answer again."
                : "The answer was not delivered: \(reason). You can answer again."
        case .expired: return "This question expired; the agent continued without it."
        case .alreadyAnswered: return "This question was already answered."
        case .answeredInSession: return "This question was already answered in the session."
        case .stillOpen(let reason):
            return reason.isEmpty ? "Still open. You can answer again." : "Still open: \(reason). You can answer again."
        case .notOpen: return "This question is not open."
        }
    }
}
