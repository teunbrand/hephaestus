// Ask Quick Look for a thumbnail and write it out as a PNG.
//
// This is the crate's one end-to-end check, and it exists because `qlmanage`
// is not usable here: `qlmanage -t` prints nothing and writes nothing for this
// type, and `qlmanage -p` / `-x -p` die with `NSInvalidArgumentException: key
// cannot be nil` for **any** data-based preview extension — Apple's own
// included. See CLAUDE.md for the control experiment.
//
// `QLThumbnailGenerator` goes through a different path and does work, so a
// real image coming back is evidence that the thumbnail extension was
// launched, read the document, rendered it and drew it. Rendering at several
// sizes is also how the small-icon cases get checked.
//
//   swiftc -O verify-thumbnail.swift -o target/verify-thumbnail
//   ./target/verify-thumbnail some.hep out.png [points] [scale]

import Foundation
import QuickLookThumbnailing
import CoreGraphics
import ImageIO
import UniformTypeIdentifiers

let arguments = CommandLine.arguments
guard arguments.count >= 2 else {
    FileHandle.standardError.write(
        "usage: verify-thumbnail <file> [out.png] [points] [scale]\n".data(using: .utf8)!
    )
    exit(2)
}
let input = URL(fileURLWithPath: arguments[1])
let output = URL(fileURLWithPath: arguments.count >= 3 ? arguments[2] : "thumbnail.png")
let points = arguments.count >= 4 ? Double(arguments[3]) ?? 800 : 800
let scale = arguments.count >= 5 ? Double(arguments[4]) ?? 2 : 2

let request = QLThumbnailGenerator.Request(
    fileAt: input,
    size: CGSize(width: points, height: points),
    scale: scale,
    // `.thumbnail` rather than `.all`: the others would let Quick Look answer
    // with a generic icon or a cached low-quality version, which would make a
    // broken extension look like a working one.
    representationTypes: .thumbnail
)

let done = DispatchSemaphore(value: 0)
var status: Int32 = 1

QLThumbnailGenerator.shared.generateBestRepresentation(for: request) { representation, error in
    defer { done.signal() }

    if let error {
        FileHandle.standardError.write(
            "no thumbnail: \(error.localizedDescription)\n".data(using: .utf8)!
        )
        return
    }
    guard let representation else {
        FileHandle.standardError.write("no thumbnail and no error\n".data(using: .utf8)!)
        return
    }

    let image = representation.cgImage
    guard let destination = CGImageDestinationCreateWithURL(
        output as CFURL, UTType.png.identifier as CFString, 1, nil
    ) else {
        FileHandle.standardError.write("cannot write \(output.path)\n".data(using: .utf8)!)
        return
    }
    CGImageDestinationAddImage(destination, image, nil)
    guard CGImageDestinationFinalize(destination) else {
        FileHandle.standardError.write("cannot encode \(output.path)\n".data(using: .utf8)!)
        return
    }

    print("thumbnail \(image.width)x\(image.height) -> \(output.path)")
    status = 0
}

// Generous: the extension has to be launched, sandboxed and asked.
if done.wait(timeout: .now() + 30) == .timedOut {
    FileHandle.standardError.write("timed out waiting for Quick Look\n".data(using: .utf8)!)
    exit(1)
}
exit(status)
