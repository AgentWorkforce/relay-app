import SwiftUI

/// The sheet that names and starts a fork. Its own view so the name, task and
/// progress it reads stay live while it is open.
struct SessionForkSheet<Source: View>: View {
    @ObservedObject var client: RelayClient
    let source: RelayAgent
    @Binding var name: String
    @Binding var task: String
    @Binding var forking: Bool
    @Binding var accepted: Bool
    @Binding var notice: String?
    @ViewBuilder let header: () -> Source
    let start: () -> Void
    let close: () -> Void

    var body: some View {
        let availability = SessionFork.availability(for: source, nodes: client.forkNodes)
        let machine = source.machine.isEmpty ? "its computer" : source.machine
        let validName = name.range(of: "^[a-z][a-z0-9-]{1,47}$", options: .regularExpression) != nil
        NavigationStack {
            Form {
                Section("Forking") {
                    header()
                        .accessibilityElement(children: .combine)
                        .accessibilityIdentifier("Fork source")
                }
                if case let .unavailable(reason) = availability {
                    Section {
                        Label(reason, systemImage: "exclamationmark.triangle").font(.callout)
                            .foregroundStyle(.orange)
                            .accessibilityIdentifier("Fork unavailable")
                    }
                }
                Section {
                    TextField("Name", text: $name)
                        .textInputAutocapitalization(.never).autocorrectionDisabled()
                        .accessibilityIdentifier("Fork name")
                    ZStack(alignment: .topLeading) {
                        if task.isEmpty {
                            Text("What should the fork do next? (optional)").foregroundStyle(.tertiary)
                                .padding(.horizontal, 5).padding(.top, 8)
                        }
                        TextEditor(text: $task).frame(minHeight: 90)
                            .scrollContentBackground(.hidden)
                            .accessibilityIdentifier("Fork task")
                    }
                } header: {
                    Text("New agent")
                } footer: {
                    Text("Starts on \(machine) in the same folder, with a copy of this whole conversation. \(source.name) keeps running on its own.")
                }
                if let notice {
                    Section { Text(notice).foregroundStyle(accepted ? Color.secondary : Color.red) }
                }
                if !accepted {
                    Section {
                        Button(action: start) {
                            HStack {
                                Spacer()
                                if forking { ProgressView().padding(.trailing, 7) }
                                Text(forking ? "Forking…" : "Fork session")
                                Spacer()
                            }
                        }
                        .disabled(forking || !validName || !availability.isAvailable)
                        .accessibilityIdentifier("Start fork")
                    } footer: {
                        if forking {
                            Text("Waiting for \(machine) to start the fork. This can take up to 2 minutes.")
                        }
                    }
                }
            }
            .navigationTitle("Fork session")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar { Button(accepted ? "Done" : "Cancel", action: close).disabled(forking) }
            .keyboardDismissToolbar()
            .scrollDismissesKeyboard(.interactively)
        }
        .frame(minWidth: 380, minHeight: 480)
        .presentationDetents([.large])
        .interactiveDismissDisabled(forking)
    }
}
