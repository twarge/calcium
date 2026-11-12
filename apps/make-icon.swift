#!/usr/bin/env swift
// Writes the app icon as an Icon Composer bundle: Fira Code's => ligature —
// the glyph the whole app is about — in warm white on the brick orange of a
// calcium flame test, which is also the editor's redefinition accent.
//
//     swift apps/make-icon.swift
//
// The output is apps/Calcium/AppIcon.icon: icon.json plus one SVG per layer.
// Xcode's asset compiler turns it into every size each platform needs and
// applies the system shape and the glass, dark, clear and tinted treatments,
// so the artwork is full bleed and carries no mask, rim or shadow of its own.
//
// The ligature is two layers, as it is two keystrokes: the `=` (Equals)
// lies on the flame and the `>` (Greater) sits in front of it, so the
// arrowhead catches its own light and drops a shadow on the bars.
//
// The bundle is committed, so this only needs running when the design
// changes. To look at the result without building:
//
//     "/Applications/Xcode.app/Contents/Applications/Icon Composer.app/Contents/Executables/ictool" \
//         apps/Calcium/AppIcon.icon --export-image --output-file icon.png \
//         --platform macOS --rendition Default --width 512 --height 512 --scale 2

import Foundation

let out = "apps/Calcium/AppIcon.icon"
guard FileManager.default.fileExists(atPath: "apps/Calcium") else {
    fatalError("run from the repository root; apps/Calcium not found")
}

// Colours. Icon Composer ignores SVG opacity and gradients inside a layer,
// so the glyph is one solid colour and the flame's shading — lit from above,
// settling into brick red — is the fill in icon.json.
let warmWhite = "#FFFAF5"
let flame = [
    "extended-srgb:0.98000,0.56000,0.31000,1.00000",
    "extended-srgb:0.78000,0.25500,0.08600,1.00000",
]

// Geometry, on the 1024 × 1024 canvas with y downward: FiraCode-Bold's `=>`
// (glyphs equal_start.seq and greater_equal_end.seq) set at 560 pt and
// centred, which is how the editor draws it and how the flat icon had it.
typealias Point = (x: Double, y: Double)
let mid = 512.0

// The `>`: one arm's end cap, then the tip. The lower arm is its mirror
// image, and the two inner edges meet at the notch between the bars.
let armInner: Point = (563.98, 370.13)
let armOuter: Point = (623.43, 319.02)
let tip: Point = (797.17, mid)
let notch: Point = (694.36, mid)

// The `=`: two bars from the left edge to the arrowhead's inner edge, and
// a little way under it so no seam shows where the layers meet.
let barLeft = 226.83
let upperBar = (top: 416.08, bottom: 486.73)
let tuck = 10.0

/// Where the arrowhead's upper inner edge crosses the height `y`.
func innerEdge(at y: Double) -> Double {
    armInner.x + (y - armInner.y) * (notch.x - armInner.x) / (notch.y - armInner.y)
}

func mirrored(_ points: [Point]) -> [Point] {
    points.reversed().map { ($0.x, 2 * mid - $0.y) }
}

func polygon(_ points: [Point]) -> String {
    let list = points.map { String(format: "%.2f,%.2f", $0.x, $0.y) }.joined(separator: " ")
    return "<polygon points=\"\(list)\" fill=\"\(warmWhite)\"/>"
}

let upperArm: [Point] = [notch, armInner, armOuter, tip]
let greater = [polygon(upperArm + mirrored(upperArm).dropFirst().dropLast())]

let upper: [Point] = [
    (barLeft, upperBar.top),
    (innerEdge(at: upperBar.top) + tuck, upperBar.top),
    (innerEdge(at: upperBar.bottom) + tuck, upperBar.bottom),
    (barLeft, upperBar.bottom),
]
let equals = [polygon(upper), polygon(mirrored(upper))]

// Layers from front to back, the order icon.json lists them. Each is its
// own group, so each is its own piece of glass.
let layers = [
    (name: "Greater", body: greater),
    (name: "Equals", body: equals),
]

func svg(_ body: [String]) -> String {
    "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"1024\" height=\"1024\" viewBox=\"0 0 1024 1024\">\n"
        + body.map { "  \($0)\n" }.joined() + "</svg>\n"
}

try FileManager.default.createDirectory(
    atPath: "\(out)/Assets", withIntermediateDirectories: true)
for layer in layers {
    try svg(layer.body).write(
        toFile: "\(out)/Assets/\(layer.name).svg", atomically: true, encoding: .utf8)
}

let document: [String: Any] = [
    "fill": ["linear-gradient": flame],
    "groups": layers.map { layer -> [String: Any] in
        [
            "layers": [["image-name": "\(layer.name).svg", "name": layer.name]],
            "shadow": ["kind": "neutral", "opacity": 0.5],
            "translucency": ["enabled": false, "value": 0.5],
        ]
    },
    "supported-platforms": ["squares": "shared"],
]
var json = try JSONSerialization.data(
    withJSONObject: document, options: [.prettyPrinted, .sortedKeys])
json.append(0x0A)
try json.write(to: URL(fileURLWithPath: "\(out)/icon.json"))
print("wrote \(out)")
