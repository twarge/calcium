import CoreGraphics
import Foundation

/// The inline chart drawn below a `plot(...)` line.
///
/// The engine has already done the mathematics — sampling, gaps, labels —
/// so this view is pure drawing: map points into a rect, stroke each
/// series, mark literal data, letter the extremes. It lives as a subview
/// of the text view, inside a gap the editor reserves with paragraph
/// spacing, so the text buffer itself stays untouched — splice, undo and
/// caret bookkeeping never learn plots exist.
///
/// One file, two thin view classes: the drawing is identical CoreGraphics
/// on both platforms and only the view plumbing differs.

/// Platform-free chart geometry.
enum PlotMath {

    /// The chart's ranges: `x` and `y` are padded so curves do not touch
    /// the frame — those map points to pixels — while `labelX` and `labelY`
    /// are the values the axes letter: the data's own extremes. A
    /// degenerate range widens instead, and its labels take the widened
    /// ends, so a flat line still has a lettered place to sit.
    static func bounds(of series: [PlotData.Series]) -> (
        x: ClosedRange<Double>, y: ClosedRange<Double>,
        labelX: ClosedRange<Double>, labelY: ClosedRange<Double>
    ) {
        var xLo = Double.infinity, xHi = -Double.infinity
        var yLo = Double.infinity, yHi = -Double.infinity
        for s in series {
            for p in s.points where p.count == 2 {
                xLo = min(xLo, p[0]); xHi = max(xHi, p[0])
                yLo = min(yLo, p[1]); yHi = max(yHi, p[1])
            }
        }
        if !xLo.isFinite { return (0...1, 0...1, 0...1, 0...1) }
        func widen(_ lo: Double, _ hi: Double) -> (ClosedRange<Double>, ClosedRange<Double>) {
            if lo < hi {
                let pad = (hi - lo) * 0.06
                return ((lo - pad)...(hi + pad), lo...hi)
            }
            let pad = max(abs(lo) * 0.5, 1)
            let widened = (lo - pad)...(hi + pad)
            return (widened, widened)
        }
        let (x, labelX) = widen(xLo, xHi)
        let (y, labelY) = widen(yLo, yHi)
        return (x, y, labelX, labelY)
    }

    /// Where a curve breaks: the engine drops unplottable samples, so a
    /// stride well past its neighbours' median is a gap, not a segment.
    static func segments(of points: [[Double]]) -> [[CGPoint]] {
        let clean = points.filter { $0.count == 2 }
        guard clean.count > 1 else {
            return clean.isEmpty ? [] : [[CGPoint(x: clean[0][0], y: clean[0][1])]]
        }
        var strides = (1..<clean.count).map { clean[$0][0] - clean[$0 - 1][0] }
        // A parametric curve doubles back on itself: x order carries no gap
        // information there, so the run stays whole.
        if strides.contains(where: { $0 < 0 }) {
            return [clean.map { CGPoint(x: $0[0], y: $0[1]) }]
        }
        strides.sort()
        let median = strides[strides.count / 2]
        var out: [[CGPoint]] = []
        var run: [CGPoint] = [CGPoint(x: clean[0][0], y: clean[0][1])]
        for index in 1..<clean.count {
            if median > 0, clean[index][0] - clean[index - 1][0] > median * 2.5 {
                out.append(run)
                run = []
            }
            run.append(CGPoint(x: clean[index][0], y: clean[index][1]))
        }
        out.append(run)
        return out
    }

    /// An axis extreme as a short label.
    static func label(_ value: Double) -> String {
        String(format: "%g", value)
    }
}

#if os(macOS)
import AppKit
import UniformTypeIdentifiers

final class PlotChartView: NSView {
    private var plot: PlotData?
    private var scale: CGFloat = 1

    /// The strip along the bottom edge that drags the chart taller.
    private static let gripHeight: CGFloat = 8
    /// Reports the proposed height, in view points, as the grip drags.
    var onResize: ((CGFloat) -> Void)?
    private var dragStart: (mouseY: CGFloat, height: CGFloat)?
    /// Where a possible drag-out began; resolved into a drag session on
    /// movement, or a caret click on release.
    private var dragOutOrigin: NSPoint?

    override var isFlipped: Bool { true }

    /// The chart takes the mouse: the grip along the bottom edge resizes,
    /// dragging the body carries the chart out of the window as an SVG
    /// file, right-clicking opens the export menu — and a plain click
    /// still lands the caret in the text, placed by hand in `mouseUp`.
    override func hitTest(_ point: NSPoint) -> NSView? {
        let local = convert(point, from: superview)
        return bounds.contains(local) ? self : nil
    }

    override func resetCursorRects() {
        addCursorRect(
            CGRect(
                x: 0, y: bounds.height - Self.gripHeight,
                width: bounds.width, height: Self.gripHeight),
            cursor: .resizeUpDown)
    }

    override func mouseDown(with event: NSEvent) {
        let local = convert(event.locationInWindow, from: nil)
        if local.y >= bounds.height - Self.gripHeight {
            dragStart = (local.y, bounds.height)
        } else {
            dragOutOrigin = local
        }
    }

    override func mouseDragged(with event: NSEvent) {
        let local = convert(event.locationInWindow, from: nil)
        if let start = dragStart {
            let height = min(max(start.height + (local.y - start.mouseY), 120), 640)
            onResize?(height)
            return
        }
        if let origin = dragOutOrigin,
            hypot(local.x - origin.x, local.y - origin.y) > 4
        {
            dragOutOrigin = nil
            beginDragOut(with: event)
        }
    }

    override func mouseUp(with event: NSEvent) {
        dragStart = nil
        if dragOutOrigin != nil {
            dragOutOrigin = nil
            placeCaret(with: event)
        }
    }

    /// A click that never became a drag still means "put the caret here":
    /// the text view no longer sees it, so the placement is done by hand.
    private func placeCaret(with event: NSEvent) {
        guard let textView = superview as? NSTextView else { return }
        let point = textView.convert(event.locationInWindow, from: nil)
        let index = textView.characterIndexForInsertion(at: point)
        textView.setSelectedRange(NSRange(location: index, length: 0))
        textView.window?.makeFirstResponder(textView)
    }

    // MARK: Dragging out

    /// The chart leaves as a file promise: Finder and friends receive an
    /// SVG, and the drag shows the chart itself.
    private func beginDragOut(with event: NSEvent) {
        guard let plot else { return }
        let provider = NSFilePromiseProvider(fileType: UTType.svg.identifier, delegate: self)
        provider.userInfo = [
            "svg": PlotExport.svg(plot, size: bounds.size),
            "name": Self.fileName(for: plot, extension: "svg"),
        ]
        let item = NSDraggingItem(pasteboardWriter: provider)
        item.setDraggingFrame(bounds, contents: NSImage(data: lightPDF()))
        beginDraggingSession(with: [item], event: event, source: self)
    }

    /// A file name from the first series' label: `height of ball(t).svg`.
    private static func fileName(for plot: PlotData, extension ext: String) -> String {
        let label = plot.series.first?.label ?? "plot"
        let cleaned = label.map { "/:".contains($0) ? "-" : $0 }
        let base = String(cleaned).trimmingCharacters(in: .whitespaces)
        return (base.isEmpty ? "plot" : base) + "." + ext
    }

    // MARK: Export

    override func menu(for event: NSEvent) -> NSMenu? {
        guard plot != nil else { return nil }
        let menu = NSMenu()
        let entries: [(String, Selector)] = [
            ("Save as SVG…", #selector(saveSVG)),
            ("Save as PDF…", #selector(savePDF)),
            ("Save as CSV…", #selector(saveCSV)),
        ]
        for (title, action) in entries {
            let item = NSMenuItem(title: title, action: action, keyEquivalent: "")
            item.target = self
            menu.addItem(item)
        }
        return menu
    }

    @objc private func saveSVG() {
        guard let plot else { return }
        save(
            Data(PlotExport.svg(plot, size: bounds.size).utf8), as: .svg,
            named: Self.fileName(for: plot, extension: "svg"))
    }

    @objc private func savePDF() {
        guard let plot else { return }
        save(lightPDF(), as: .pdf, named: Self.fileName(for: plot, extension: "pdf"))
    }

    @objc private func saveCSV() {
        guard let plot else { return }
        save(
            Data(PlotExport.csv(plot).utf8), as: .commaSeparatedText,
            named: Self.fileName(for: plot, extension: "csv"))
    }

    /// Vector, through the same draw pass — pinned to the light appearance
    /// so exports match the SVG, not the window theme.
    private func lightPDF() -> Data {
        let saved = appearance
        appearance = NSAppearance(named: .aqua)
        let data = dataWithPDF(inside: bounds)
        appearance = saved
        return data
    }

    private func save(_ data: Data, as type: UTType, named name: String) {
        guard let window else { return }
        let panel = NSSavePanel()
        panel.allowedContentTypes = [type]
        panel.nameFieldStringValue = name
        panel.beginSheetModal(for: window) { response in
            guard response == .OK, let url = panel.url else { return }
            try? data.write(to: url)
        }
    }

    func render(_ plot: PlotData, scale: CGFloat) {
        self.plot = plot
        self.scale = scale
        needsDisplay = true
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        needsDisplay = true
    }

    override func draw(_ dirtyRect: NSRect) {
        guard let plot, let context = NSGraphicsContext.current?.cgContext else { return }
        let s = min(max(scale, 0.5), 4)
        // The system's caption size, scaled with the type it sits amongst.
        let small = NSFont.systemFont(
            ofSize: NSFont.preferredFont(forTextStyle: .caption1).pointSize * s)
        let ink = NSColor.labelColor
        PlotDrawing.draw(
            plot, in: bounds, context: context, fontScale: s,
            frameColor: ink.withAlphaComponent(0.12).cgColor,
            gridColor: ink.withAlphaComponent(0.25).cgColor,
            fillColor: ink.withAlphaComponent(0.04).cgColor,
            palette: Palette.series.map(\.cgColor),
            text: { string, colour in
                let resolved = colour.flatMap { NSColor(cgColor: $0) } ?? .secondaryLabelColor
                return NSAttributedString(
                    string: string,
                    attributes: [.font: small, .foregroundColor: resolved])
            })
    }
}

extension PlotChartView: NSDraggingSource {
    nonisolated func draggingSession(
        _ session: NSDraggingSession, sourceOperationMaskFor context: NSDraggingContext
    ) -> NSDragOperation {
        .copy
    }
}

extension PlotChartView: NSFilePromiseProviderDelegate {
    /// Both callbacks read only the strings frozen into `userInfo` when
    /// the drag began, so a chart that re-renders mid-drag cannot change
    /// what lands.
    nonisolated func filePromiseProvider(
        _ filePromiseProvider: NSFilePromiseProvider, fileNameForType fileType: String
    ) -> String {
        let info = filePromiseProvider.userInfo as? [String: String]
        return info?["name"] ?? "plot.svg"
    }

    nonisolated func filePromiseProvider(
        _ filePromiseProvider: NSFilePromiseProvider, writePromiseTo url: URL,
        completionHandler: @escaping (Error?) -> Void
    ) {
        let info = filePromiseProvider.userInfo as? [String: String]
        do {
            try (info?["svg"] ?? "").write(to: url, atomically: true, encoding: .utf8)
            completionHandler(nil)
        } catch {
            completionHandler(error)
        }
    }
}
#else
import UIKit

final class PlotChartView: UIView {
    private var plot: PlotData?

    override init(frame: CGRect) {
        super.init(frame: frame)
        isUserInteractionEnabled = false
        isOpaque = false
        contentMode = .redraw
        // Dynamic colours resolve at draw time; redraw when the theme flips.
        registerForTraitChanges([UITraitUserInterfaceStyle.self]) {
            (view: PlotChartView, _) in view.setNeedsDisplay()
        }
    }

    required init?(coder: NSCoder) { fatalError("unused") }

    func render(_ plot: PlotData, scale: CGFloat) {
        self.plot = plot
        setNeedsDisplay()
    }

    override func draw(_ rect: CGRect) {
        guard let plot, let context = UIGraphicsGetCurrentContext() else { return }
        // The system caption style, which follows Dynamic Type.
        let small = UIFont.preferredFont(forTextStyle: .caption1)
        let ink = UIColor.label
        PlotDrawing.draw(
            plot, in: bounds, context: context, fontScale: 1,
            frameColor: ink.withAlphaComponent(0.12).cgColor,
            gridColor: ink.withAlphaComponent(0.25).cgColor,
            fillColor: ink.withAlphaComponent(0.04).cgColor,
            palette: PaletteIOS.series.map(\.cgColor),
            text: { string, colour in
                NSAttributedString(
                    string: string,
                    attributes: [
                        .font: small,
                        .foregroundColor: colour.map(UIColor.init(cgColor:)) ?? UIColor.secondaryLabel,
                    ])
            })
    }
}
#endif

/// The drawing itself, shared verbatim by both views: everything is CGPath
/// and attributed-string drawing, with colours and fonts handed in.
enum PlotDrawing {

    static func draw(
        _ plot: PlotData,
        in bounds: CGRect,
        context: CGContext,
        fontScale: CGFloat,
        frameColor: CGColor,
        gridColor: CGColor,
        fillColor: CGColor,
        palette: [CGColor],
        text: (String, CGColor?) -> NSAttributedString
    ) {
        let inset: CGFloat = 2
        let labelHeight = 13 * fontScale
        let yGutter = 42 * fontScale
        let area = CGRect(
            x: bounds.minX + inset + yGutter,
            y: bounds.minY + inset,
            width: bounds.width - 2 * inset - yGutter,
            height: bounds.height - 2 * inset - labelHeight)
        guard area.width > 20, area.height > 20 else { return }

        let (xRange, yRange, labelX, labelY) = PlotMath.bounds(of: plot.series)
        let map = { (p: CGPoint) -> CGPoint in
            CGPoint(
                x: area.minX + area.width
                    * (p.x - xRange.lowerBound) / (xRange.upperBound - xRange.lowerBound),
                y: area.maxY - area.height
                    * (p.y - yRange.lowerBound) / (yRange.upperBound - yRange.lowerBound))
        }

        // The card: a whisper of fill and a hairline, enough to say where
        // the chart is without shouting over the text.
        let card = CGPath(
            roundedRect: area, cornerWidth: 4, cornerHeight: 4, transform: nil)
        context.setFillColor(fillColor)
        context.addPath(card)
        context.fillPath()
        context.setStrokeColor(frameColor)
        context.setLineWidth(1)
        context.addPath(card)
        context.strokePath()

        // Zero lines, dashed, where zero is on the chart.
        context.saveGState()
        context.addPath(card)
        context.clip()
        context.setStrokeColor(gridColor)
        context.setLineWidth(0.5)
        context.setLineDash(phase: 0, lengths: [2, 3])
        if yRange.contains(0) {
            let y = map(CGPoint(x: xRange.lowerBound, y: 0)).y
            context.move(to: CGPoint(x: area.minX, y: y))
            context.addLine(to: CGPoint(x: area.maxX, y: y))
        }
        if xRange.contains(0) {
            let x = map(CGPoint(x: 0, y: yRange.lowerBound)).x
            context.move(to: CGPoint(x: x, y: area.minY))
            context.addLine(to: CGPoint(x: x, y: area.maxY))
        }
        context.strokePath()
        context.setLineDash(phase: 0, lengths: [])

        // The series: a line per segment, and marks when the points are
        // literal data rather than a sampled sweep.
        for (index, series) in plot.series.enumerated() {
            let colour = palette[index % palette.count]
            context.setStrokeColor(colour)
            context.setFillColor(colour)
            context.setLineWidth(1.5)
            context.setLineJoin(.round)
            for segment in PlotMath.segments(of: series.points) {
                guard let first = segment.first else { continue }
                if segment.count > 1 {
                    context.move(to: map(first))
                    for point in segment.dropFirst() {
                        context.addLine(to: map(point))
                    }
                    context.strokePath()
                }
                if !series.swept {
                    for point in segment {
                        let at = map(point)
                        context.fillEllipse(
                            in: CGRect(x: at.x - 2.5, y: at.y - 2.5, width: 5, height: 5))
                    }
                }
            }
        }
        context.restoreGState()

        // The extremes, lettered small: the data's own bounds, set where
        // they map — the frame's breathing room stays unlabeled, so a
        // sweep over -1..1 reads -1 and 1, not the padded frame.
        let yTop = text(PlotMath.label(labelY.upperBound), nil)
        let yTopAt = map(CGPoint(x: labelX.lowerBound, y: labelY.upperBound)).y
        yTop.draw(
            at: CGPoint(
                x: area.minX - yTop.size().width - 4,
                y: min(max(yTopAt - yTop.size().height / 2, area.minY), area.maxY)))
        let yBottom = text(PlotMath.label(labelY.lowerBound), nil)
        let yBottomAt = map(CGPoint(x: labelX.lowerBound, y: labelY.lowerBound)).y
        yBottom.draw(
            at: CGPoint(
                x: area.minX - yBottom.size().width - 4,
                y: min(
                    max(yBottomAt - yBottom.size().height / 2, area.minY),
                    area.maxY - yBottom.size().height)))
        let xLeft = text(PlotMath.label(labelX.lowerBound), nil)
        xLeft.draw(
            at: CGPoint(
                x: max(map(CGPoint(x: labelX.lowerBound, y: labelY.lowerBound)).x, area.minX),
                y: area.maxY + 2))
        let xRight = text(PlotMath.label(labelX.upperBound), nil)
        xRight.draw(
            at: CGPoint(
                x: min(
                    map(CGPoint(x: labelX.upperBound, y: labelY.lowerBound)).x, area.maxX)
                    - xRight.size().width,
                y: area.maxY + 2))
        if let title = PlotExport.xTitle(of: plot) {
            let name = text(title, nil)
            name.draw(
                at: CGPoint(x: area.midX - name.size().width / 2, y: area.maxY + 2))
        }

        // The vertical axis's unit, top left inside the card — the
        // legend's counterpart in the opposite corner.
        if let unit = plot.yUnit {
            let name = text(unit, nil)
            name.draw(at: CGPoint(x: area.minX + 6, y: area.minY + 4))
        }

        // The legend: each series named in its own colour, top right,
        // inside the card.
        var legendY = area.minY + 4
        for (index, series) in plot.series.enumerated() {
            let entry = text(series.label, palette[index % palette.count])
            entry.draw(
                at: CGPoint(x: area.maxX - entry.size().width - 6, y: legendY))
            legendY += entry.size().height + 1
        }
    }
}

/// Chart exports: the same layout spoken as standalone SVG, and the
/// sampled points as CSV. Platform-free strings; the views wrap them in
/// save panels.
enum PlotExport {

    /// The x-axis title, unit and all: `t (s)`.
    static func xTitle(of plot: PlotData) -> String? {
        guard let x = plot.x else { return nil }
        return plot.xUnit.map { "\(x) (\($0))" } ?? x
    }

    /// The series palette as hex — the light column of the same preference
    /// the views draw with, since SVG is always light-on-transparent.
    private static var palette: [String] {
        ColorRole.series.map { $0.hex(dark: false) }
    }

    /// The chart as a standalone SVG — always light-on-transparent, the
    /// same geometry as the inline drawing at font scale 1.
    static func svg(_ plot: PlotData, size: CGSize) -> String {
        let width = max(size.width, 200), height = max(size.height, 120)
        let inset: CGFloat = 2, labelHeight: CGFloat = 13, yGutter: CGFloat = 42
        let area = CGRect(
            x: inset + yGutter, y: inset,
            width: width - 2 * inset - yGutter, height: height - 2 * inset - labelHeight)
        let (xRange, yRange, labelX, labelY) = PlotMath.bounds(of: plot.series)
        func mapX(_ x: Double) -> CGFloat {
            area.minX + area.width
                * CGFloat((x - xRange.lowerBound) / (xRange.upperBound - xRange.lowerBound))
        }
        func mapY(_ y: Double) -> CGFloat {
            area.maxY - area.height
                * CGFloat((y - yRange.lowerBound) / (yRange.upperBound - yRange.lowerBound))
        }
        func num(_ value: CGFloat) -> String { String(format: "%.2f", value) }
        func escape(_ text: String) -> String {
            text.replacingOccurrences(of: "&", with: "&amp;")
                .replacingOccurrences(of: "<", with: "&lt;")
                .replacingOccurrences(of: ">", with: "&gt;")
        }
        func label(_ text: String, x: CGFloat, y: CGFloat, anchor: String, colour: String = "#6e6e73")
            -> String
        {
            "<text x=\"\(num(x))\" y=\"\(num(y))\" text-anchor=\"\(anchor)\" "
                + "dominant-baseline=\"hanging\" fill=\"\(colour)\">\(escape(text))</text>\n"
        }

        var out = "<svg xmlns=\"http://www.w3.org/2000/svg\" "
        out += "width=\"\(Int(width))\" height=\"\(Int(height))\" "
        out += "viewBox=\"0 0 \(Int(width)) \(Int(height))\" "
        out += "font-family=\"system-ui, -apple-system, Helvetica, sans-serif\" font-size=\"10\">\n"

        // The card, and a clip so curves stay inside it.
        let card = "x=\"\(num(area.minX))\" y=\"\(num(area.minY))\" "
            + "width=\"\(num(area.width))\" height=\"\(num(area.height))\" rx=\"4\""
        out += "<defs><clipPath id=\"area\"><rect \(card)/></clipPath></defs>\n"
        out += "<rect \(card) fill=\"rgba(0,0,0,0.04)\" stroke=\"rgba(0,0,0,0.12)\"/>\n"

        out += "<g clip-path=\"url(#area)\">\n"
        // Zero lines, dashed, where zero is on the chart.
        if yRange.contains(0) {
            let y = num(mapY(0))
            out += "<line x1=\"\(num(area.minX))\" y1=\"\(y)\" x2=\"\(num(area.maxX))\" y2=\"\(y)\" "
                + "stroke=\"rgba(0,0,0,0.25)\" stroke-width=\"0.5\" stroke-dasharray=\"2 3\"/>\n"
        }
        if xRange.contains(0) {
            let x = num(mapX(0))
            out += "<line x1=\"\(x)\" y1=\"\(num(area.minY))\" x2=\"\(x)\" y2=\"\(num(area.maxY))\" "
                + "stroke=\"rgba(0,0,0,0.25)\" stroke-width=\"0.5\" stroke-dasharray=\"2 3\"/>\n"
        }
        // The series: a polyline per segment, marks for literal data.
        for (index, series) in plot.series.enumerated() {
            let colour = palette[index % palette.count]
            for segment in PlotMath.segments(of: series.points) {
                if segment.count > 1 {
                    let points = segment
                        .map { "\(num(mapX($0.x))),\(num(mapY($0.y)))" }
                        .joined(separator: " ")
                    out += "<polyline points=\"\(points)\" fill=\"none\" stroke=\"\(colour)\" "
                        + "stroke-width=\"1.5\" stroke-linejoin=\"round\"/>\n"
                }
                if !series.swept {
                    for point in segment {
                        out += "<circle cx=\"\(num(mapX(point.x)))\" cy=\"\(num(mapY(point.y)))\" "
                            + "r=\"2.5\" fill=\"\(colour)\"/>\n"
                    }
                }
            }
        }
        out += "</g>\n"

        // The extremes — the data's own bounds, set where they map — the
        // axis title, and the legend, as the view draws them.
        out += label(
            PlotMath.label(labelY.upperBound), x: area.minX - 4,
            y: max(mapY(labelY.upperBound) - 5, area.minY), anchor: "end")
        out += label(
            PlotMath.label(labelY.lowerBound), x: area.minX - 4,
            y: min(mapY(labelY.lowerBound) - 5, area.maxY - 11), anchor: "end")
        out += label(
            PlotMath.label(labelX.lowerBound), x: max(mapX(labelX.lowerBound), area.minX),
            y: area.maxY + 2, anchor: "start")
        out += label(
            PlotMath.label(labelX.upperBound), x: min(mapX(labelX.upperBound), area.maxX),
            y: area.maxY + 2, anchor: "end")
        if let title = xTitle(of: plot) {
            out += label(title, x: area.midX, y: area.maxY + 2, anchor: "middle")
        }
        if let unit = plot.yUnit {
            out += label(unit, x: area.minX + 6, y: area.minY + 4, anchor: "start")
        }
        var legendY = area.minY + 4
        for (index, series) in plot.series.enumerated() {
            out += label(
                series.label, x: area.maxX - 6, y: legendY, anchor: "end",
                colour: palette[index % palette.count])
            legendY += 12
        }
        out += "</svg>\n"
        return out
    }

    /// The sampled points as CSV: one x column when every series shares the
    /// same sample positions, and long form — series, x, y — when not.
    static func csv(_ plot: PlotData) -> String {
        func field(_ text: String) -> String {
            if text.contains(",") || text.contains("\"") || text.contains("\n") {
                return "\"" + text.replacingOccurrences(of: "\"", with: "\"\"") + "\""
            }
            return text
        }
        let xHead = xTitle(of: plot) ?? "x"
        let unitTail = plot.yUnit.map { " (\($0))" } ?? ""
        let columns = plot.series.map { series in series.points.filter { $0.count == 2 } }
        guard let first = columns.first else { return "" }
        var rows: [String] = []
        let aligned = columns.allSatisfy { column in
            column.count == first.count
                && zip(column, first).allSatisfy { $0[0] == $1[0] }
        }
        if aligned {
            rows.append(
                ([xHead] + plot.series.map { $0.label + unitTail }).map(field)
                    .joined(separator: ","))
            for index in first.indices {
                let cells = ["\(first[index][0])"] + columns.map { "\($0[index][1])" }
                rows.append(cells.joined(separator: ","))
            }
        } else {
            rows.append(["series", xHead, "y" + unitTail].map(field).joined(separator: ","))
            for (series, column) in zip(plot.series, columns) {
                for point in column {
                    rows.append([field(series.label), "\(point[0])", "\(point[1])"].joined(separator: ","))
                }
            }
        }
        return rows.joined(separator: "\n") + "\n"
    }
}
