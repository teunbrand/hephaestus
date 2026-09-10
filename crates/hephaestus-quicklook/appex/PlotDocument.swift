// The one place either extension talks to Rust, and the one place a PDF
// becomes pixels.
//
// Shared by both bundles rather than duplicated: `PreviewProvider` hands the
// PDF to Quick Look untouched, and `ThumbnailProvider` rasterizes it through
// Core Graphics. Neither needs a rasterizer of its own — which is the whole
// reason a thumbnail costs no new Rust at all.

import CoreGraphics
import Foundation

/// Writes a line into the extension's own sandbox container.
///
/// Compiled in only with `-D HEP_QL_DIAG`, which `build-appex.sh` passes when
/// `HEPHAESTUS_QL_DIAG=1`. It exists because a sandboxed appex has almost no
/// way to report anything: the unified log needs privileges to read, `print`
/// goes nowhere, and a failure surfaces only as Finder showing a generic icon.
/// The container path is in `CLAUDE.md`.
@inline(__always)
func diag(_ message: @autoclosure () -> String) {
    #if HEP_QL_DIAG
    let url = FileManager.default.temporaryDirectory
        .appendingPathComponent("hep-ql-diag.txt")
    guard let data = (message() + "\n").data(using: .utf8) else { return }
    if let handle = try? FileHandle(forWritingTo: url) {
        handle.seekToEndOfFile()
        handle.write(data)
        try? handle.close()
    } else {
        try? data.write(to: url)
    }
    #endif
}

/// A plot document rendered to PDF.
struct PlotDocument {
    /// The PDF bytes.
    let data: Data
    /// The page size in points, which is also the PDF's MediaBox.
    let size: CGSize

    /// Read and render the document at `url`.
    ///
    /// Throws `CocoaError.fileReadCorruptFile` when the bytes are not a
    /// document this build can read — which is what makes Quick Look fall back
    /// to its generic icon rather than showing an empty panel.
    static func read(at url: URL) throws -> PlotDocument {
        // `mappedIfSafe` because a document carrying embedded images can be
        // large, and this is a preview rather than an open.
        let bytes = try Data(contentsOf: url, options: .mappedIfSafe)

        let rendered: HepPdf = bytes.withUnsafeBytes { raw in
            hephaestus_pdf_from_document(
                raw.bindMemory(to: UInt8.self).baseAddress,
                raw.count
            )
        }
        diag("read \(bytes.count) bytes from \(url.lastPathComponent)")
        guard let start = rendered.data else {
            diag("unreadable document")
            throw CocoaError(.fileReadCorruptFile)
        }
        diag("rendered \(rendered.len) pdf bytes at \(rendered.width_pt)x\(rendered.height_pt)")
        // Copied out before the Rust allocation is released, so the result
        // owns its bytes.
        defer { hephaestus_pdf_free(rendered) }
        return PlotDocument(
            data: Data(bytes: start, count: rendered.len),
            size: CGSize(width: rendered.width_pt, height: rendered.height_pt)
        )
    }

    /// Draw page one into `context`, scaled to fill `target`.
    ///
    /// Returns whether anything was drawn; a `false` from a thumbnail reply's
    /// drawing block is how Quick Look is told to fall back.
    ///
    /// `document` is held for the whole call deliberately — a `CGPDFPage` does
    /// not keep its document alive, and drawing a page whose document has been
    /// released is a use-after-free.
    func draw(into context: CGContext, fitting target: CGRect) -> Bool {
        guard let provider = CGDataProvider(data: data as CFData),
              let document = CGPDFDocument(provider),
              let page = document.page(at: 1)
        else {
            diag("cannot open the rendered pdf")
            return false
        }

        let media = page.getBoxRect(.mediaBox)
        guard media.width > 0, media.height > 0,
              target.width > 0, target.height > 0
        else {
            return false
        }
        let scale = min(target.width / media.width, target.height / media.height)

        context.saveGState()
        // Centre it, so a page whose aspect differs from the target is
        // letterboxed rather than pinned to a corner.
        context.translateBy(
            x: target.minX + (target.width - media.width * scale) / 2,
            y: target.minY + (target.height - media.height * scale) / 2
        )
        context.scaleBy(x: scale, y: scale)
        context.translateBy(x: -media.origin.x, y: -media.origin.y)
        context.drawPDFPage(page)
        context.restoreGState()
        return true
    }

    /// The area of `context` that can actually be drawn into, in user space.
    ///
    /// **Not derivable from what Quick Look was asked for.** A thumbnail
    /// reply's bitmap comes back at `contextSize × request.scale` pixels, but
    /// the context arrives with an *identity* CTM — so drawing in the points
    /// that `contextSize` was expressed in covers a quarter of it at 2×, in
    /// the bottom-left corner. Asking the context how big it is sidesteps the
    /// question entirely and is correct under either convention.
    ///
    /// `fallback` is used where the clip is unbounded, which a context with no
    /// clip path reports.
    static func drawableArea(of context: CGContext, fallback: CGSize) -> CGRect {
        let clip = context.boundingBoxOfClipPath
        if clip.isNull || clip.isInfinite || clip.width < 1 || clip.height < 1 {
            return CGRect(origin: .zero, size: fallback)
        }
        return clip
    }

    /// The largest box with this document's aspect that fits inside `bounds`.
    ///
    /// Quick Look accepts a context smaller than the maximum it asked for and
    /// centres it, so reporting the true aspect is better than filling the box
    /// and distorting — or than padding, which would make the thumbnail's
    /// visible edges wrong in Finder's gallery view.
    func aspectFit(in bounds: CGSize) -> CGSize {
        guard size.width > 0, size.height > 0,
              bounds.width > 0, bounds.height > 0
        else {
            return bounds
        }
        let scale = min(bounds.width / size.width, bounds.height / size.height)
        return CGSize(
            width: max(1, (size.width * scale).rounded()),
            height: max(1, (size.height * scale).rounded())
        )
    }
}
