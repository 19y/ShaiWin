// Minimal AppKit skeleton for the macOS shell.
//
// What it does today: puts a menu-bar icon in place, creates a borderless
// transparent panel across the top of the main screen, and draws the rope using
// the same layout maths as the Windows build.
//
// What is missing (see ../README.md): enumerating and moving other apps' windows,
// snapshots, the card hit-testing, the journal, and the permission prompts.
//
// UNVERIFIED: written on Windows, never compiled with Xcode.

import AppKit
import ShaiWinCore

/// A click-through panel pinned across the top of one screen.
final class LinePanel: NSPanel {
    private let layout: LineLayout

    init(screen: NSScreen) {
        let frame = screen.frame
        let width = frame.width * screen.backingScaleFactor
        let height = 196 * screen.backingScaleFactor
        self.layout = LineLayout(width: width, scale: screen.backingScaleFactor)
        super.init(
            contentRect: NSRect(x: frame.minX, y: frame.maxY - 196, width: frame.width, height: 196),
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )
        isOpaque = false
        backgroundColor = .clear
        hasShadow = false
        level = .statusBar
        collectionBehavior = [.canJoinAllSpaces, .stationary, .ignoresCycle, .fullScreenAuxiliary]
        ignoresMouseEvents = false // the view decides per point, like WM_NCHITTEST
    }

    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }

    override func draw(_ dirtyRect: NSRect) {
        guard let context = NSGraphicsContext.current?.cgContext else { return }
        let width = bounds.width
        let scale = layout.panelHeight / 196

        // The rope: a shallow quadratic sag, three passes like the Windows build.
        context.setStrokeColor(NSColor.black.withAlphaComponent(0.22).cgColor)
        context.setLineWidth(2.2 * scale)
        context.move(to: CGPoint(x: -20, y: bounds.height - 10))
        context.addQuadCurve(
            to: CGPoint(x: width + 20, y: bounds.height - 10),
            control: CGPoint(x: width / 2, y: bounds.height - 10 - 2 * layout.ropeY(width / 2) + 2 * layout.ropeY(0))
        )
        context.strokePath()

        // Hint text while the line is empty.
        let text = "把窗口拖到这里 · Drag a window up here" as NSString
        let attributes: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: 12),
            .foregroundColor: NSColor.secondaryLabelColor,
        ]
        let size = text.size(withAttributes: attributes)
        text.draw(
            at: NSPoint(x: (width - size.width) / 2, y: bounds.height - 70),
            withAttributes: attributes
        )
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    private var statusItem: NSStatusItem?
    private var panel: LinePanel?

    func applicationDidFinishLaunching(_ notification: Notification) {
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.squareLength)
        statusItem?.button?.image = NSImage(systemSymbolName: "hanger", accessibilityDescription: "ShaiWin")
        statusItem?.menu = buildMenu()

        if let screen = NSScreen.main {
            panel = LinePanel(screen: screen)
            panel?.orderFrontRegardless()
        }

        // TODO(mac): ask for Accessibility + Screen Recording permission here,
        // explaining why, and show a first-run window if either is missing.
    }

    private func buildMenu() -> NSMenu {
        let menu = NSMenu()
        // TODO(mac): "Hang the window I am using" — needs the focused window's
        // AXUIElement, which in turn needs Accessibility permission.
        menu.addItem(withTitle: "挂起当前窗口", action: nil, keyEquivalent: "")
        menu.addItem(.separator())
        menu.addItem(withTitle: "全部取下", action: nil, keyEquivalent: "")
        menu.addItem(withTitle: "显示/隐藏晾衣架", action: #selector(toggleLine), keyEquivalent: "t")
        menu.addItem(.separator())
        menu.addItem(withTitle: "退出晒窗", action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q")
        return menu
    }

    @objc private func toggleLine() {
        guard let panel else { return }
        if panel.isVisible {
            panel.orderOut(nil)
        } else {
            panel.orderFrontRegardless()
        }
    }
}

let app = NSApplication.shared
let delegate = AppDelegate()
app.delegate = delegate
app.setActivationPolicy(.accessory)
app.run()
