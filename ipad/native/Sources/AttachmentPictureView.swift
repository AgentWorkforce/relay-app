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
/// earlier identity session is never drawn.
struct AttachmentPicture<Fallback: View>: View {
    let attachment: RelayAttachment
    let context: AttachmentPictureContext
    @ViewBuilder let fallback: () -> Fallback
    @State private var loaded: (image: UIImage, generation: Int)?
    @State private var failedGeneration: Int?

    private struct LoadKey: Equatable {
        let fileID: String
        let generation: Int
    }

    var body: some View {
        Group {
            if failedGeneration == context.generation || context.store.showsButton(attachment) {
                fallback()
            } else if let image = shownImage {
                picture(image)
            } else {
                placeholder
            }
        }
        .task(id: LoadKey(fileID: attachment.id, generation: context.generation)) {
            let generation = context.generation
            guard loaded?.generation != generation, failedGeneration != generation else { return }
            let image = await context.store.load(attachment, resolve: context.resolve)
            guard !Task.isCancelled, generation == context.generation else { return }
            // No picture: it failed, or the store was cleared under it.
            // Either way the row falls back to the download button.
            if let image { loaded = (image, generation) } else { failedGeneration = generation }
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
