// The macOS Quick Look preview extension: press space on a `.hep` in Finder
// and this is what draws it.
//
// It is deliberately tiny, and that is the point of the approach. Since macOS
// 12 Quick Look accepts a *data* reply rather than a view, so an extension
// that can produce a PDF needs no UI code, no view controller and no layout —
// and `hephaestus`'s renderer-free `document-read,pdf` build produces exactly
// that with no GPU, which an extension has no business opening.
//
// PDF is the right reply type rather than a convenient one: that backend
// embeds every glyph it draws, so a preview looks right on a machine with none
// of the plot's fonts, and Quick Look scales vector content to whatever size
// the panel is. It scales rather than reflowing; `CLAUDE.md` explains why the
// API leaves no other option.
//
// Everything that makes this *load* is in `Preview-Info.plist`, not here. See
// `CLAUDE.md`; the short version is that `QLIsDataBasedPreview` is required
// and undocumented.

import Foundation
import QuickLookUI
import UniformTypeIdentifiers

/// Hands a plot document to Quick Look as a PDF.
///
/// Named in `Preview-Info.plist` as `NSExtensionPrincipalClass`, which is
/// resolved with `NSClassFromString` — so the `@objc` name is given explicitly
/// to make that a plain class name rather than Swift's mangled
/// `_TtC19HephaestusQuickLook15PreviewProvider`. A miss there fails silently.
@objc(PreviewProvider)
final class PreviewProvider: QLPreviewProvider, QLPreviewingController {
    /// The completion-handler form of the one `QLPreviewingController`
    /// requirement that matters here.
    ///
    /// Written this way rather than as Swift `async` because it is the
    /// Objective-C contract Quick Look actually invokes by selector, and being
    /// explicit about it costs nothing. Both forms emit the selector — that
    /// was checked with `otool -s __TEXT __objc_methname` rather than assumed,
    /// so `async` is a free choice here and not a trap.
    func providePreview(
        for request: QLFilePreviewRequest,
        completionHandler handler: @escaping (QLPreviewReply?, Error?) -> Void
    ) {
        do {
            let plot = try PlotDocument.read(at: request.fileURL)
            let data = plot.data
            handler(
                QLPreviewReply(dataOfContentType: .pdf, contentSize: plot.size) { _ in data },
                nil
            )
        } catch {
            // An unreadable document reports an error, which is what makes
            // Quick Look fall back to its generic icon — the right
            // degradation, and better than a blank panel that reads as a hang.
            handler(nil, error)
        }
    }
}
