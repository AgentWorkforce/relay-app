import Observation

/// Runs `apply` with what `read` returns, now and again after any observed
/// value `read` used changes, while started. Changes made in one main-actor
/// turn run it once. Only `read` is tracked, so `apply` may write the
/// models freely.
///
/// A model starts a reaction while what it feeds is on screen and stops it
/// when that goes away, so nothing is recomputed for a hidden view.
@MainActor
final class ModelReaction<Value> {
    private let read: @MainActor () -> Value
    private let apply: @MainActor (Value) -> Void
    /// Renewed by every start and stop: a change noticed for an earlier run
    /// does nothing.
    private var generation = 0
    private(set) var isRunning = false
    /// How many times `read` ran; tests count recomputations with it.
    private(set) var runs = 0

    init(read: @escaping @MainActor () -> Value, apply: @escaping @MainActor (Value) -> Void) {
        self.read = read
        self.apply = apply
    }

    func start() {
        guard !isRunning else { return }
        isRunning = true
        generation &+= 1
        run(generation)
    }

    func stop() {
        guard isRunning else { return }
        isRunning = false
        generation &+= 1
    }

    private func run(_ current: Int) {
        guard isRunning, current == generation else { return }
        runs += 1
        let value = withObservationTracking(read) { [weak self] in
            // Called before the change lands: read again once it has.
            Task { @MainActor [weak self] in self?.run(current) }
        }
        apply(value)
    }
}
