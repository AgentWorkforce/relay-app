import SwiftUI

/// What a chat row needs to draw posted pictures: the store, the identity
/// session they belong to, how to get a file's download URL and how to open
/// the file (what the download button does).
struct AttachmentPictureContext {
    let store: AttachmentImageStore
    /// `RelayClient.sessionGeneration` when the row was drawn.
    let generation: Int
    let resolve: AttachmentImageStore.Resolve
    let open: @MainActor (RelayAttachment) -> Void
}

/// A posted image drawn as a picture: a placeholder of the picture's size
/// while it loads, the picture (a tap opens the file), or `fallback` (the
/// download button) when it cannot be shown. A picture loaded under an
/// earlier identity session is never drawn. A load that failed for a
/// reason a retry may change shows the button and tries again the next
/// time the row appears; a load dropped by `clear()` tries again at once.
struct AttachmentPicture<Fallback: View>: View {
    let attachment: RelayAttachment
    let context: AttachmentPictureContext
    @ViewBuilder let fallback: () -> Fallback
    @State private var loaded: (image: UIImage, generation: Int)?
    /// The session whose last try failed: the button shows until a later
    /// try (the next appearance) gives the picture.
    @State private var failedGeneration: Int?

    private struct LoadKey: Equatable {
        let fileID: String
        let generation: Int
    }

    /// Loads tried in a row while each is dropped, before the button shows.
    private static var attempts: Int { 3 }

    var body: some View {
        Group {
            if context.store.showsButton(attachment) {
                fallback()
            } else if let image = shownImage {
                picture(image)
            } else if failedGeneration == context.generation {
                fallback()
            } else {
                placeholder
            }
        }
        .task(id: LoadKey(fileID: attachment.id, generation: context.generation)) {
            let generation = context.generation
            guard loaded?.generation != generation else { return }
            for _ in 0..<Self.attempts {
                let result = await context.store.load(attachment, resolve: context.resolve)
                guard !Task.isCancelled, generation == context.generation else { return }
                switch result {
                case .image(let image):
                    loaded = (image, generation)
                    failedGeneration = nil
                    return
                case .failed:
                    failedGeneration = generation
                    return
                case .dropped:
                    // The store was cleared under it; not a failure.
                    continue
                }
            }
            failedGeneration = generation
        }
    }

    /// The loaded picture of this session, or the store's cached one.
    @MainActor private var shownImage: UIImage? {
        if let loaded, loaded.generation == context.generation { return loaded.image }
        return context.store.cached(attachment)
    }

    @MainActor private func picture(_ image: UIImage) -> some View {
        let aspect = image.size.height > 0 ? max(image.size.width / image.size.height, 0.05) : 1
        return Button {
            context.open(attachment)
        } label: {
            Image(uiImage: image)
                .resizable()
                .aspectRatio(aspect, contentMode: .fit)
                .frame(maxWidth: AttachmentPreview.maximumHeight * aspect)
                .clipShape(RoundedRectangle(cornerRadius: 8))
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .buttonStyle(.plain)
        .accessibilityLabel(attachment.filename)
        .accessibilityAddTraits(.isImage)
        .accessibilityHint("Opens the file")
        .accessibilityIdentifier("Attachment picture")
    }

    private var placeholder: some View {
        RoundedRectangle(cornerRadius: 8)
            .fill(Color(.systemBackground).opacity(0.55))
            .frame(height: AttachmentPreview.maximumHeight)
            .frame(maxWidth: .infinity)
            .overlay { ProgressView() }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("Loading \(attachment.filename)")
            .accessibilityIdentifier("Attachment picture loading")
    }
}
