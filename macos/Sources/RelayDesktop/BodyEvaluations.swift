/// Counts body evaluations of the views that mark themselves, so tests can
/// prove a model change re-renders only the views that show it. Release
/// builds count nothing.
@MainActor
enum BodyEvaluations {
    private(set) static var counts: [String: Int] = [:]

    @inline(__always)
    static func record(_ view: String) {
        #if DEBUG
        counts[view, default: 0] += 1
        #endif
    }

    static func count(_ view: String) -> Int { counts[view] ?? 0 }
}
