// A thin Swift wrapper over the C ABI exported by crates/ffi.
//
// This is the layer that a macOS shell (AppKit or SwiftUI) talks to. It owns the
// memory returned by Rust and exposes values in the units AppKit uses (points,
// seconds), while the core works in device pixels and seconds.
//
// UNVERIFIED: written for the macOS port and not compiled on Windows.

import CShaiWin
import CoreGraphics

/// Layout for one line on one screen. `width` is in device pixels, `scale` is
/// the backing scale factor (`screen.backingScaleFactor` on Retina).
public final class LineLayout {
    private let handle: UnsafeMutableRawPointer

    public init(width: CGFloat, scale: CGFloat) {
        self.handle = shaiwin_layout_new(Float(width), Float(scale))
    }

    deinit {
        shaiwin_layout_free(handle)
    }

    /// Centre x of the card at `index` when `count` cards hang on the line.
    public func slotX(index: Int, count: Int) -> CGFloat {
        CGFloat(shaiwin_layout_slot_x(handle, UInt(index), UInt(count)))
    }

    /// Screen x of the rope at `x`.
    public func ropeY(_ x: CGFloat) -> CGFloat {
        CGFloat(shaiwin_layout_rope_y(handle, Float(x)))
    }

    /// Height of the strip the overlay needs, in device pixels.
    public var panelHeight: CGFloat {
        CGFloat(shaiwin_layout_panel_height(handle))
    }
}

/// A card's flight between a window rectangle and its slot on the line.
public final class Flight {
    private let handle: UnsafeMutableRawPointer

    public init(from: CGPoint, to: CGPoint, lift: CGFloat, duration: TimeInterval) {
        self.handle = shaiwin_flight_new(
            Float(from.x), Float(from.y),
            Float(to.x), Float(to.y),
            Float(lift), Float(duration)
        )
    }

    deinit {
        shaiwin_flight_free(handle)
    }

    /// Advances by `dt` seconds; returns `true` while still in the air.
    @discardableResult
    public func step(_ dt: TimeInterval) -> Bool {
        shaiwin_flight_step(handle, Float(dt)) != 0
    }

    public var position: CGPoint {
        var out = [Float](repeating: 0, count: 2)
        out.withUnsafeMutableBufferPointer { buffer in
            shaiwin_flight_position(handle, buffer.baseAddress)
        }
        return CGPoint(x: CGFloat(out[0]), y: CGFloat(out[1]))
    }
}

/// What a card needs to show. The shell fills this from CGWindowList + AppKit.
public struct HungWindow: Identifiable {
    public let id: CGWindowID
    public let pid: pid_t
    public let title: String
    public let appName: String
    /// Accent colour, derived from `appName` in Rust once the theme API is
    /// exported; for now pick from a local table with the same hashing rule.
    public let accent: NSColor
    public let snapshot: CGImage?

    public init(id: CGWindowID, pid: pid_t, title: String, appName: String, accent: NSColor, snapshot: CGImage?) {
        self.id = id
        self.pid = pid
        self.title = title
        self.appName = appName
        self.accent = accent
        self.snapshot = snapshot
    }
}
