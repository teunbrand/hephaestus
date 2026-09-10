// The macOS Quick Look thumbnail extension: what a `.hep` looks like as an
// icon in Finder, rather than as a space-bar preview.
//
// A separate bundle from the preview, and not by choice — an app extension
// declares exactly one `NSExtensionPointIdentifier`, and previews and
// thumbnails are different points. Everything below the two principal classes
// is shared (`PlotDocument.swift`), so the duplication is two Info.plists
// rather than two implementations.
//
// The interesting difference from the preview is that **this one is told what
// size to render.** `QLFileThumbnailRequest` carries `maximumSize` and
// `scale`, where `QLFilePreviewRequest` carries nothing but a URL — so a
// thumbnail is rasterized to fit exactly, while a preview has to pick a size
// and be scaled.

import CoreGraphics
import Foundation
import QuickLookThumbnailing

/// Rasterizes a plot document into a Finder thumbnail.
///
/// Named in `Info.plist` as `NSExtensionPrincipalClass`, resolved with
/// `NSClassFromString` — hence the explicit `@objc` name rather than Swift's
/// mangled one.
@objc(ThumbnailProvider)
final class ThumbnailProvider: QLThumbnailProvider {
    override func provideThumbnail(
        for request: QLFileThumbnailRequest,
        _ handler: @escaping (QLThumbnailReply?, Error?) -> Void
    ) {
        do {
            let plot = try PlotDocument.read(at: request.fileURL)
            // The reply's size carries the plot's aspect: Quick Look accepts a
            // context smaller than the maximum it asked for and centres it, so
            // reporting the true aspect beats filling the box and distorting.
            let box = plot.aspectFit(in: request.maximumSize)
            handler(
                QLThumbnailReply(contextSize: box) { context -> Bool in
                    // The context's own bounds rather than `box` — see
                    // `PlotDocument.drawableArea`, which is where the
                    // points-versus-pixels trap lives.
                    let target = PlotDocument.drawableArea(of: context, fallback: box)
                    diag("thumbnail: box \(box), drawable \(target)")
                    return plot.draw(into: context, fitting: target)
                },
                nil
            )
        } catch {
            handler(nil, error)
        }
    }
}
