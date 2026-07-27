#!/usr/bin/env swift
// Renders the app icon: Fira Code's => ligature — the glyph the whole app
// is about — set in warm white on the brick orange of a calcium flame
// test, which is also the editor's redefinition accent.
//
//     swift apps/make-icon.swift
//
// writes AppIcon-1024.png into the asset catalog. Full-bleed square: both
// platforms apply their own mask.

import AppKit
import CoreText
import UniformTypeIdentifiers

let side = 1024
let repoRelative = "apps/Calcium"
let fontPath = "\(repoRelative)/Resources/Fonts/FiraCode-Bold.ttf"
let outPath = "\(repoRelative)/Assets.xcassets/AppIcon.appiconset/AppIcon-1024.png"

guard FileManager.default.fileExists(atPath: fontPath) else {
    fatalError("run from the repository root; \(fontPath) not found")
}
CTFontManagerRegisterFontsForURL(
    URL(fileURLWithPath: fontPath) as CFURL, .process, nil)

let space = CGColorSpace(name: CGColorSpace.sRGB)!
let ctx = CGContext(
    data: nil, width: side, height: side, bitsPerComponent: 8, bytesPerRow: 0,
    space: space, bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)!

// The flame: lit from above, settling into brick red.
let gradient = CGGradient(
    colorsSpace: space,
    colors: [
        CGColor(srgbRed: 0.980, green: 0.560, blue: 0.310, alpha: 1),
        CGColor(srgbRed: 0.780, green: 0.255, blue: 0.086, alpha: 1),
    ] as CFArray,
    locations: [0, 1])!
ctx.drawLinearGradient(
    gradient,
    start: CGPoint(x: CGFloat(side) / 2, y: CGFloat(side)),
    end: CGPoint(x: CGFloat(side) / 2, y: 0),
    options: [])

// `=>` with contextual alternates on — the default — so CoreText hands
// back the single ⇒ glyph, exactly as the editor draws it.
let font = CTFontCreateWithName("FiraCode-Bold" as CFString, 560, nil)
let text = NSAttributedString(string: "=>", attributes: [
    NSAttributedString.Key(kCTFontAttributeName as String): font,
    NSAttributedString.Key(kCTForegroundColorAttributeName as String):
        CGColor(srgbRed: 1.0, green: 0.980, blue: 0.960, alpha: 1),
])
let line = CTLineCreateWithAttributedString(text)
let bounds = CTLineGetBoundsWithOptions(line, [.useGlyphPathBounds])

ctx.setShadow(
    offset: CGSize(width: 0, height: -14), blur: 36,
    color: CGColor(srgbRed: 0.25, green: 0.05, blue: 0, alpha: 0.35))
ctx.textPosition = CGPoint(
    x: (CGFloat(side) - bounds.width) / 2 - bounds.minX,
    y: (CGFloat(side) - bounds.height) / 2 - bounds.minY)
CTLineDraw(line, ctx)

let image = ctx.makeImage()!
try? FileManager.default.createDirectory(
    atPath: (outPath as NSString).deletingLastPathComponent,
    withIntermediateDirectories: true)
let dest = CGImageDestinationCreateWithURL(
    URL(fileURLWithPath: outPath) as CFURL, UTType.png.identifier as CFString, 1, nil)!
CGImageDestinationAddImage(dest, image, nil)
CGImageDestinationFinalize(dest)
print("wrote \(outPath)")
