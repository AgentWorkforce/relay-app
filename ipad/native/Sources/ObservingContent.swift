import SwiftUI

/// Content that is built again whenever `object` publishes a change.
///
/// A view pushed with `navigationDestination` is built by a closure that the
/// navigation stack keeps. Changes to the presenting view's `@State` reach
/// the pushed view, but a change published by an object the presenting view
/// observes does not reliably make the stack build it again, so the pushed
/// view can keep showing what that object held before. Wrapping the pushed
/// content in this view makes it observe the object itself.
struct ObservingContent<Object: ObservableObject, Content: View>: View {
    @ObservedObject var object: Object
    @ViewBuilder let content: () -> Content

    var body: some View {
        content()
    }
}
