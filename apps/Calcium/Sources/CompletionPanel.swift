#if os(macOS)
import AppKit

/// The completion menu: a borderless floating panel under the word being
/// typed, listing names with their current values. Never becomes key — the
/// text view keeps focus and routes ↑ ↓ Tab Return Esc here from its
/// `doCommandBy` while the panel shows.
///
/// Type size is per document, so the menu owns no fixed dimensions: rows,
/// insets and width are recomputed from the current font each time it shows.
@MainActor
final class CompletionPanel: NSObject, NSTableViewDataSource, NSTableViewDelegate {
    private let panel: NSPanel
    private let table = NSTableView()
    private var items: [Completion] = []
    private var scale: CGFloat = 1
    /// Leading inset of each row's text; the panel is placed so this lines
    /// up with the first character of the word being completed.
    private var pad: CGFloat = 6
    private var topInset: NSLayoutConstraint!
    private var bottomInset: NSLayoutConstraint!
    var onPick: ((Completion) -> Void)?

    var isVisible: Bool { panel.isVisible }
    var current: Completion? {
        items.indices.contains(table.selectedRow) ? items[table.selectedRow] : nil
    }

    override init() {
        panel = NSPanel(
            contentRect: NSRect(x: 0, y: 0, width: 360, height: 100),
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered, defer: true)
        panel.level = .floating
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.hidesOnDeactivate = true
        super.init()

        let background = NSVisualEffectView()
        background.material = .menu
        background.state = .active
        background.wantsLayer = true
        background.layer?.cornerRadius = 8
        background.layer?.masksToBounds = true

        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("name"))
        table.addTableColumn(column)
        table.headerView = nil
        table.style = .plain
        // Rows carry all the spacing themselves, so the panel's height is
        // exactly rows × rowHeight and never drifts from the math in `show`.
        table.intercellSpacing = .zero
        table.backgroundColor = .clear
        table.dataSource = self
        table.delegate = self
        table.action = #selector(rowClicked)
        table.target = self

        let scroll = NSScrollView()
        scroll.documentView = table
        scroll.hasVerticalScroller = false
        scroll.drawsBackground = false
        scroll.translatesAutoresizingMaskIntoConstraints = false
        background.addSubview(scroll)
        topInset = scroll.topAnchor.constraint(equalTo: background.topAnchor, constant: 4)
        bottomInset = scroll.bottomAnchor.constraint(
            equalTo: background.bottomAnchor, constant: -4)
        NSLayoutConstraint.activate([
            topInset, bottomInset,
            scroll.leadingAnchor.constraint(equalTo: background.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: background.trailingAnchor),
        ])
        panel.contentView = background
    }

    func show(_ items: [Completion], below anchor: NSRect, scale: CGFloat) {
        self.items = items
        self.scale = scale

        // Every dimension follows from the scaled font's line height, so
        // the menu keeps its proportions at any zoom instead of clipping
        // tall glyphs against sizes tuned for 1×.
        let line = ceil(NSLayoutManager().defaultLineHeight(for: Typography.body(scale)))
        pad = ceil(line * 0.4)
        let inset = ceil(line * 0.2)
        table.rowHeight = line + ceil(line * 0.3)
        topInset.constant = inset
        bottomInset.constant = -inset
        table.reloadData()

        let widest = items.map { rowText(for: $0).size().width }.max() ?? 0
        var width = ceil(widest) + pad * 2
        let height = CGFloat(items.count) * table.rowHeight + inset * 2
        // `anchor` is in screen coordinates, the first rect of the word
        // being completed; the menu hangs just below its line, its text
        // aligned under the word. When the screen runs out below — likely
        // once the type is large — it opens upward from the line instead.
        var x = anchor.minX - pad
        var top = anchor.minY - 2
        let screen = NSScreen.screens.first {
            NSPointInRect(NSPoint(x: anchor.minX, y: anchor.midY), $0.frame)
        } ?? NSScreen.main
        if let visible = screen?.visibleFrame {
            width = min(width, floor(visible.width * 0.8))
            x = min(max(x, visible.minX), visible.maxX - width)
            if top - height < visible.minY, anchor.maxY + 2 + height <= visible.maxY {
                top = anchor.maxY + 2 + height
            }
        }
        table.tableColumns[0].width = width
        panel.setContentSize(NSSize(width: width, height: height))
        panel.setFrameTopLeftPoint(NSPoint(x: x, y: top))
        panel.orderFront(nil)
    }

    func hide() {
        panel.orderOut(nil)
        items = []
    }

    func move(_ delta: Int) {
        guard !items.isEmpty else { return }
        let row = min(max(0, table.selectedRow + delta), items.count - 1)
        table.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
        table.scrollRowToVisible(row)
    }

    /// Tab's motion. Nothing is lit when the menu opens — the author may
    /// simply be typing a word the menu happens to match — so the first
    /// press lights the first row, and each further press steps down,
    /// wrapping past the end. `selectedRow` is −1 while nothing is lit,
    /// which the arithmetic folds into the first step.
    func cycle() {
        guard !items.isEmpty else { return }
        let row = (table.selectedRow + 1) % items.count
        table.selectRowIndexes(IndexSet(integer: row), byExtendingSelection: false)
        table.scrollRowToVisible(row)
    }

    @objc private func rowClicked() {
        guard let pick = current else { return }
        onPick?(pick)
    }

    /// One row's text: the name in the label colour, the value trailing in
    /// the secondary — both in the editor's own font at its current size.
    private func rowText(for item: Completion) -> NSAttributedString {
        let font = Typography.body(scale)
        let text = NSMutableAttributedString(
            string: item.name,
            attributes: [.font: font, .foregroundColor: NSColor.labelColor])
        if !item.value.isEmpty {
            text.append(
                NSAttributedString(
                    string: "   " + item.value,
                    attributes: [.font: font, .foregroundColor: NSColor.secondaryLabelColor]))
        }
        return text
    }

    // MARK: Table plumbing

    func numberOfRows(in tableView: NSTableView) -> Int { items.count }

    func tableView(
        _ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int
    ) -> NSView? {
        let field = NSTextField(labelWithAttributedString: rowText(for: items[row]))
        field.lineBreakMode = .byTruncatingTail
        field.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        // Centred in the row, so the breathing room rowHeight adds surrounds
        // the glyphs instead of leaving them pinned to the top edge.
        let cell = NSView()
        field.translatesAutoresizingMaskIntoConstraints = false
        cell.addSubview(field)
        NSLayoutConstraint.activate([
            field.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            field.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: pad),
            field.trailingAnchor.constraint(
                lessThanOrEqualTo: cell.trailingAnchor, constant: -pad),
        ])
        return cell
    }
}
#endif
