import SwiftUI
import UIKit

/// Lets people put the keyboard away wherever a text field raised it: a
/// keyboard-down button in the keyboard toolbar, and a tap on the message area.
enum KeyboardDismiss {
    /// Resigns whichever text field is first responder, including fields with
    /// no `FocusState` binding such as the terminal composer.
    static func dismiss() {
        UIApplication.shared.sendAction(#selector(UIResponder.resignFirstResponder), to: nil, from: nil, for: nil)
    }
}

/// A keyboard-down button at the trailing end of the keyboard toolbar. Use it
/// only in a sheet whose own navigation stack pushes no other text fields
/// (new agent, new message, summary search). A `.keyboard` toolbar at the root
/// of the phone's main stack also applied to the conversation pushed on top,
/// and its bar covered the message composer; fields there use an inline
/// Hide keyboard button instead.
struct KeyboardDismissToolbar: ViewModifier {
    func body(content: Content) -> some View {
        content.toolbar {
            ToolbarItemGroup(placement: .keyboard) {
                Spacer()
                Button { KeyboardDismiss.dismiss() } label: {
                    Image(systemName: "keyboard.chevron.compact.down")
                }
                .accessibilityLabel("Hide keyboard")
                .accessibilityIdentifier("Keyboard dismiss")
            }
        }
    }
}

extension View {
    func keyboardDismissToolbar() -> some View {
        modifier(KeyboardDismissToolbar())
    }

    /// Puts a tap target behind this view. A tap on empty space hides the
    /// keyboard; a tap on a bubble, link or button inside never reaches it,
    /// because the target is a sibling behind the content, not a gesture on
    /// an ancestor that could compete with the content's own handling.
    func dismissesKeyboardOnBackgroundTap() -> some View {
        background(Color.clear.contentShape(Rectangle()).onTapGesture { KeyboardDismiss.dismiss() })
    }
}
