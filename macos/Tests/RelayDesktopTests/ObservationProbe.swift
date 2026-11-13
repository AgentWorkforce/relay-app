import Observation
import SwiftUI
@testable import RelayDesktop

/// Counts the changes observers of `read` are told about: every observed
/// value `read` uses that a model assigns, as a view reading them would be
/// invalidated. `values` keeps what `read` returned after each change.
@MainActor
final class ObservationProbe<Value> {
    private let read: @MainActor () -> Value
    private(set) var changes = 0
    private(set) var values: [Value] = []
    private var stopped = false

    init(_ read: @escaping @MainActor () -> Value) {
        self.read = read
        track()
    }

    func stop() { stopped = true }

    private func track() {
        guard !stopped else { return }
        _ = withObservationTracking(read) { [weak self] in
            MainActor.assumeIsolated { self?.changes += 1 }
            Task { @MainActor [weak self] in
                guard let self, !self.stopped else { return }
                self.values.append(self.read())
                self.track()
            }
        }
    }
}

/// Reports what `windowIsVisible` tells the views inside a window.
struct VisibilityProbe: View {
    @Environment(\.windowIsVisible) private var windowIsVisible
    let report: (Bool) -> Void

    var body: some View {
        Color.clear.frame(width: 1, height: 1)
            .onAppear { report(windowIsVisible) }
            .onChange(of: windowIsVisible) { report($1) }
    }
}
