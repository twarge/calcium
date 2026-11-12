#!/usr/bin/env swift
// Writes the app icon: Fira Code's => ligature — the glyph the whole app is
// about — in warm white on the brick orange of a calcium flame test, which
// is also the editor's redefinition accent.
//
//     swift apps/make-icon.swift
//
// The icon is layered, and each system composes the layers itself, so the
// artwork is full bleed and carries no mask, rim or shadow of its own. The
// ligature is two layers, as it is two keystrokes: the `=` lies on the
// flame and the `>` sits in front of it.
//
// There are two outputs, one picture:
//
//   apps/Calcium/AppIcon.icon — an Icon Composer bundle for macOS and iOS:
//   icon.json plus one SVG per layer. The asset compiler makes every size
//   from it and applies the system shape and the glass, dark, clear and
//   tinted treatments.
//
//   apps/Calcium/Assets.xcassets/AppIcon.solidimagestack — the same layers
//   as images for visionOS, which Icon Composer does not cover. The system
//   crops them to a circle and spreads them in depth.
//
// Both are committed, so this only needs running when the design changes.
// To look at the Icon Composer result without building:
//
//     "/Applications/Xcode.app/Contents/Applications/Icon Composer.app/Contents/Executables/ictool" \
//         apps/Calcium/AppIcon.icon --export-image --output-file icon.png \
//         --platform macOS --rendition Default --width 512 --height 512 --scale 2

import CoreGraphics
import Foundation
import ImageIO
import UniformTypeIdentifiers

let composerIcon = "apps/Calcium/AppIcon.icon"
let visionIcon = "apps/Calcium/Assets.xcassets/AppIcon.solidimagestack"
guard FileManager.default.fileExists(atPath: "apps/Calcium") else {
    fatalError("run from the repository root; apps/Calcium not found")
}

// Colours, as sRGB components. The glyph is one solid colour: Icon Composer
// ignores opacity and gradients inside a layer. The flame is lit from above
// and settles into brick red.
typealias Colour = (r: Double, g: Double, b: Double)
let warmWhite: Colour = (1.0, 0.980, 0.960)
let flameTop: Colour = (0.980, 0.560, 0.310)
let flameBottom: Colour = (0.780, 0.255, 0.086)

// Geometry, on the 1024 × 1024 canvas with y downward: FiraCode-Bold's `=>`
// (glyphs equal_start.seq and greater_equal_end.seq) set at 560 pt and
// centred, which is how the editor draws it and how the flat icon had it.
typealias Point = (x: Double, y: Double)
let side = 1024
let mid = 512.0

// The `>`: one arm's end cap, then the tip. The lower arm is its mirror
// image, and the two inner edges meet at the notch between the bars.
let armInner: Point = (563.98, 370.13)
let armOuter: Point = (623.43, 319.02)
let tip: Point = (797.17, mid)
let notch: Point = (694.36, mid)

// The `=`: two bars from the left edge to the arrowhead's inner edge.
let barLeft = 226.83
let upperBar = (top: 416.08, bottom: 486.73)

/// Where the arrowhead's upper inner edge crosses the height `y`.
func innerEdge(at y: Double) -> Double {
    armInner.x + (y - armInner.y) * (notch.x - armInner.x) / (notch.y - armInner.y)
}

func mirrored(_ points: [Point]) -> [Point] {
    points.reversed().map { ($0.x, 2 * mid - $0.y) }
}

let upperArm: [Point] = [notch, armInner, armOuter, tip]
let greater = [upperArm + mirrored(upperArm).dropFirst().dropLast()]

/// The bars, running `tuck` under the arrowhead so that no seam shows
/// where the two layers meet.
func equals(tuck: Double) -> [[Point]] {
    let upper: [Point] = [
        (barLeft, upperBar.top),
        (innerEdge(at: upperBar.top) + tuck, upperBar.top),
        (innerEdge(at: upperBar.bottom) + tuck, upperBar.bottom),
        (barLeft, upperBar.bottom),
    ]
    return [upper, mirrored(upper)]
}

func write(json: Any, to path: String) throws {
    var data = try JSONSerialization.data(
        withJSONObject: json, options: [.prettyPrinted, .sortedKeys])
    data.append(0x0A)
    try data.write(to: URL(fileURLWithPath: path))
}

// MARK: Icon Composer, for macOS and iOS

func svg(_ polygons: [[Point]]) -> String {
    let fill = String(
        format: "#%02X%02X%02X", Int((warmWhite.r * 255).rounded()),
        Int((warmWhite.g * 255).rounded()), Int((warmWhite.b * 255).rounded()))
    let body = polygons.map { points -> String in
        let list = points.map { String(format: "%.2f,%.2f", $0.x, $0.y) }.joined(separator: " ")
        return "  <polygon points=\"\(list)\" fill=\"\(fill)\"/>\n"
    }
    return "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"\(side)\" height=\"\(side)\" "
        + "viewBox=\"0 0 \(side) \(side)\">\n" + body.joined() + "</svg>\n"
}

func extendedSRGB(_ c: Colour) -> String {
    String(format: "extended-srgb:%.5f,%.5f,%.5f,1.00000", c.r, c.g, c.b)
}

// Layers from front to back, the order icon.json lists them. Each is its
// own group, so each is its own piece of glass: the arrowhead catches its
// own light and drops a shadow on the bars. Glass can turn translucent, so
// the bars tuck only a little way under it.
let glass = [
    (name: "Greater", polygons: greater),
    (name: "Equals", polygons: equals(tuck: 10)),
]

try FileManager.default.createDirectory(
    atPath: "\(composerIcon)/Assets", withIntermediateDirectories: true)
for layer in glass {
    try svg(layer.polygons).write(
        toFile: "\(composerIcon)/Assets/\(layer.name).svg", atomically: true, encoding: .utf8)
}
try write(
    json: [
        "fill": ["linear-gradient": [extendedSRGB(flameTop), extendedSRGB(flameBottom)]],
        "groups": glass.map { layer -> [String: Any] in
            [
                "layers": [["image-name": "\(layer.name).svg", "name": layer.name]],
                "shadow": ["kind": "neutral", "opacity": 0.5],
                "translucency": ["enabled": false, "value": 0.5],
            ]
        },
        "supported-platforms": ["squares": "shared"],
    ], to: "\(composerIcon)/icon.json")
print("wrote \(composerIcon)")

// MARK: Solid image stack, for visionOS

let sRGB = CGColorSpace(name: CGColorSpace.sRGB)!

func cgColour(_ c: Colour) -> CGColor {
    CGColor(colorSpace: sRGB, components: [c.r, c.g, c.b, 1])!
}

/// A square image drawn with y downward, as the geometry has it.
func image(opaque: Bool, draw: (CGContext) -> Void) -> CGImage {
    let alpha: CGImageAlphaInfo = opaque ? .noneSkipLast : .premultipliedLast
    let ctx = CGContext(
        data: nil, width: side, height: side, bitsPerComponent: 8, bytesPerRow: 0,
        space: sRGB, bitmapInfo: alpha.rawValue)!
    ctx.translateBy(x: 0, y: CGFloat(side))
    ctx.scaleBy(x: 1, y: -1)
    draw(ctx)
    return ctx.makeImage()!
}

func glyph(_ polygons: [[Point]]) -> CGImage {
    image(opaque: false) { ctx in
        ctx.setFillColor(cgColour(warmWhite))
        for points in polygons {
            ctx.addLines(between: points.map { CGPoint(x: $0.x, y: $0.y) })
            ctx.closePath()
        }
        ctx.fillPath()
    }
}

let flame = image(opaque: true) { ctx in
    let gradient = CGGradient(
        colorsSpace: sRGB, colors: [cgColour(flameTop), cgColour(flameBottom)] as CFArray,
        locations: [0, 1])!
    ctx.drawLinearGradient(
        gradient, start: CGPoint(x: mid, y: 0), end: CGPoint(x: mid, y: CGFloat(side)),
        options: [])
}

// Layers from front to back, the order the stack lists them. The back one
// must be opaque. These layers are, too, and they shift against each other
// as the head moves, so here the bars run well under the arrowhead.
let solid = [
    (name: "Front", image: glyph(greater)),
    (name: "Middle", image: glyph(equals(tuck: 40))),
    (name: "Back", image: flame),
]

let info = ["author": "xcode", "version": 1] as [String: Any]
try? FileManager.default.removeItem(atPath: visionIcon)
for layer in solid {
    let folder = "\(visionIcon)/\(layer.name).solidimagestacklayer"
    try FileManager.default.createDirectory(
        atPath: "\(folder)/Content.imageset", withIntermediateDirectories: true)
    try write(json: ["info": info], to: "\(folder)/Contents.json")
    try write(
        json: [
            "images": [["filename": "\(layer.name).png", "idiom": "vision", "scale": "2x"]],
            "info": info,
        ], to: "\(folder)/Content.imageset/Contents.json")
    let file = URL(fileURLWithPath: "\(folder)/Content.imageset/\(layer.name).png")
    let png = CGImageDestinationCreateWithURL(
        file as CFURL, UTType.png.identifier as CFString, 1, nil)!
    CGImageDestinationAddImage(png, layer.image, nil)
    guard CGImageDestinationFinalize(png) else { fatalError("could not write \(file.path)") }
}
try write(
    json: [
        "info": info,
        "layers": solid.map { ["filename": "\($0.name).solidimagestacklayer"] },
    ], to: "\(visionIcon)/Contents.json")
try write(
    json: ["info": info],
    to: (visionIcon as NSString).deletingLastPathComponent + "/Contents.json")
print("wrote \(visionIcon)")
