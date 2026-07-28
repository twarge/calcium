#if os(macOS)
import AppKit

/// The completion menu: a borderless floating panel under the word being
/// typed, listing names with their current values. Never becomes key — the
/// text view keeps focus and routes ↑ ↓ Tab Return Esc here from its
/// `doCommandBy` while the panel shows.
@MainActor
final class CompletionPanel: NSObject, NSTableViewDataSource, NSTableViewDelegate {
    private let panel: NSPanel
    private let table = NSTableView()
    private var items: [Completion] = []
    private var scale: CGFloat = 1
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
        table.rowHeight = 22
        table.style = .plain
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
        NSLayoutConstraint.activate([
            scroll.topAnchor.constraint(equalTo: background.topAnchor, constant: 4),
            scroll.bottomAnchor.constraint(equalTo: background.bottomAnchor, constant: -4),
            scroll.leadingAnchor.constraint(equalTo: background.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: background.trailingAnchor),
        ])
        panel.contentView = background
    }

    func show(_ items: [Completion], below anchor: NSRect, scale: CGFloat) {
        self.items = items
        self.scale = scale
        table.reloadData()
        table.selectRowIndexes(IndexSet(integer: 0), byExtendingSelection: false)
        let height = CGFloat(items.count) * table.rowHeight + 10
        panel.setContentSize(NSSize(width: 360, height: height))
        // `anchor` is in screen coordinates, the first rect of the word
        // being completed; the menu hangs just below its line.
        panel.setFrameTopLeftPoint(NSPoint(x: anchor.minX - 6, y: anchor.minY - 2))
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

    @objc private func rowClicked() {
        guard let pick = current else { return }
        onPick?(pick)
    }

    // MARK: Table plumbing

    func numberOfRows(in tableView: NSTableView) -> Int { items.count }

    func tableView(
        _ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int
    ) -> NSView? {
        let item = items[row]
        let text = NSMutableAttributedString(
            string: item.name,
            attributes: [
                .font: Typography.body(scale),
                .foregroundColor: NSColor.labelColor,
            ])
        if !item.value.isEmpty {
            text.append(
                NSAttributedString(
                    string: "   " + item.value,
                    attributes: [
                        .font: Typography.body(scale),
                        .foregroundColor: NSColor.secondaryLabelColor,
                    ]))
        }
        let field = NSTextField(labelWithAttributedString: text)
        field.lineBreakMode = .byTruncatingTail
        return field
    }
}
#endif
