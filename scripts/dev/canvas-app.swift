// A custom-drawn Cocoa window for strobe's image anchors and pixel waits (dev fixture, not shipped): one
// NSView that draws the tiles itself and exposes no accessibility children, so only its pixels say what it
// shows. The same tiles as canvas-app.cs: a click on PLAY or Space counts (one bar each), a click on STOP or R
// resets, X shows or hides a square. Its content fills the frame (a transparent title bar), so the tiles sit
// where a native target's client area (the frame on macOS) puts them.
//   swiftc -O scripts/dev/canvas-app.swift -o /tmp/strobe-canvas/canvas-app
//   /tmp/strobe-canvas/canvas-app --anchors /tmp/strobe-canvas   (each tile as a PNG, PLAY at 1.5x and 2x too)
import AppKit

enum Tiles {
    static let play = NSRect(x: 40, y: 40, width: 160, height: 60)
    static let stop = NSRect(x: 240, y: 40, width: 160, height: 60)
    static let itemA = NSRect(x: 40, y: 160, width: 120, height: 50)
    static let itemB = NSRect(x: 240, y: 160, width: 120, height: 50)
    static let square = NSRect(x: 440, y: 160, width: 100, height: 100)

    static func rgb(_ r: CGFloat, _ g: CGFloat, _ b: CGFloat) -> NSColor { NSColor(srgbRed: r / 255, green: g / 255, blue: b / 255, alpha: 1) }

    static func text(_ s: String, _ x: CGFloat, _ y: CGFloat) {
        (s as NSString).draw(at: NSPoint(x: x, y: y), withAttributes: [.font: NSFont.boldSystemFont(ofSize: 14), .foregroundColor: NSColor.white])
    }

    // Each tile at its own origin, top-left, in points (the view is flipped).
    static func drawPlay() {
        rgb(40, 160, 80).setFill(); NSRect(x: 0, y: 0, width: 160, height: 60).fill()
        let border = NSBezierPath(rect: NSRect(x: 6, y: 6, width: 148, height: 48)); border.lineWidth = 3; NSColor.white.setStroke(); border.stroke()
        let tri = NSBezierPath(); tri.move(to: NSPoint(x: 24, y: 16)); tri.line(to: NSPoint(x: 24, y: 44)); tri.line(to: NSPoint(x: 48, y: 30)); tri.close()
        rgb(16, 48, 16).setFill(); tri.fill()
        text("PLAY", 70, 20)
    }

    static func drawStop() {
        rgb(190, 50, 50).setFill(); NSRect(x: 0, y: 0, width: 160, height: 60).fill()
        rgb(48, 16, 16).setFill(); NSRect(x: 22, y: 16, width: 28, height: 28).fill()
        text("STOP", 70, 20)
    }

    static func drawItem() {
        rgb(40, 80, 190).setFill(); NSRect(x: 0, y: 0, width: 120, height: 50).fill()
        rgb(240, 210, 40).setFill(); NSBezierPath(ovalIn: NSRect(x: 45, y: 10, width: 30, height: 30)).fill()
    }

    static func drawSquare() {
        rgb(240, 140, 30).setFill(); NSRect(x: 0, y: 0, width: 100, height: 100).fill()
        let d = NSBezierPath(); d.move(to: NSPoint(x: 50, y: 15)); d.line(to: NSPoint(x: 85, y: 50)); d.line(to: NSPoint(x: 50, y: 85)); d.line(to: NSPoint(x: 15, y: 50)); d.close()
        NSColor.black.setFill(); d.fill()
    }

    static func drawMissing() {
        rgb(150, 40, 170).setFill(); NSRect(x: 0, y: 0, width: 80, height: 40).fill()
        NSColor.white.setFill(); NSBezierPath(ovalIn: NSRect(x: 25, y: 5, width: 30, height: 30)).fill()
    }

    /// `draw` at `scale` into a PNG of the tile's size.
    static func save(_ path: String, _ w: CGFloat, _ h: CGFloat, _ scale: CGFloat, _ draw: () -> Void) {
        let rep = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int((w * scale).rounded()), pixelsHigh: Int((h * scale).rounded()), bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true, isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)!
        rep.size = NSSize(width: w, height: h)
        NSGraphicsContext.saveGraphicsState()
        let ctx = NSGraphicsContext(bitmapImageRep: rep)!
        NSGraphicsContext.current = ctx
        // Flipped, as the view draws.
        ctx.cgContext.translateBy(x: 0, y: h)
        ctx.cgContext.scaleBy(x: 1, y: -1)
        NSGraphicsContext.current = NSGraphicsContext(cgContext: ctx.cgContext, flipped: true)
        draw()
        NSGraphicsContext.restoreGraphicsState()
        try! rep.representation(using: .png, properties: [:])!.write(to: URL(fileURLWithPath: path))
    }
}

final class Canvas: NSView {
    var count = 0
    var square = false
    override var isFlipped: Bool { true }
    override var acceptsFirstResponder: Bool { true }

    func at(_ r: NSRect, _ draw: () -> Void) {
        NSGraphicsContext.saveGraphicsState()
        let t = NSAffineTransform(); t.translateX(by: r.minX, yBy: r.minY); t.concat()
        draw()
        NSGraphicsContext.restoreGraphicsState()
    }

    override func draw(_ dirty: NSRect) {
        Tiles.rgb(32, 40, 48).setFill(); bounds.fill()
        at(Tiles.play, Tiles.drawPlay)
        at(Tiles.stop, Tiles.drawStop)
        at(Tiles.itemA, Tiles.drawItem)
        at(Tiles.itemB, Tiles.drawItem)
        if square { at(Tiles.square, Tiles.drawSquare) }
        NSColor.white.setFill()
        for i in 0..<min(count, 40) { NSRect(x: 40 + CGFloat(i) * 12, y: 270, width: 8, height: 40).fill() }
    }

    override func mouseDown(with e: NSEvent) {
        let p = convert(e.locationInWindow, from: nil)
        if Tiles.play.contains(p) { count += 1 } else if Tiles.stop.contains(p) { count = 0 } else { return }
        needsDisplay = true
    }

    override func keyDown(with e: NSEvent) {
        switch e.charactersIgnoringModifiers {
        case " ": count += 1
        case "r": count = 0
        case "x": square.toggle()
        default: return super.keyDown(with: e)
        }
        needsDisplay = true
    }
}

let args = CommandLine.arguments
if args.count == 3, args[1] == "--anchors" {
    let d = args[2]
    try? FileManager.default.createDirectory(atPath: d, withIntermediateDirectories: true)
    Tiles.save("\(d)/play.png", 160, 60, 1, Tiles.drawPlay)
    Tiles.save("\(d)/play@1.5x.png", 160, 60, 1.5, Tiles.drawPlay)
    Tiles.save("\(d)/play@2x.png", 160, 60, 2, Tiles.drawPlay)
    Tiles.save("\(d)/stop.png", 160, 60, 1, Tiles.drawStop)
    Tiles.save("\(d)/item.png", 120, 50, 1, Tiles.drawItem)
    Tiles.save("\(d)/square.png", 100, 100, 1, Tiles.drawSquare)
    Tiles.save("\(d)/missing.png", 80, 40, 1, Tiles.drawMissing)
    exit(0)
}

final class App: NSObject, NSApplicationDelegate {
    let window = NSWindow(contentRect: NSRect(x: 200, y: 200, width: 600, height: 360), styleMask: [.titled, .closable, .fullSizeContentView], backing: .buffered, defer: false)
    let canvas = Canvas()

    func applicationDidFinishLaunching(_ note: Notification) {
        window.title = "Canvas fixture"
        window.titlebarAppearsTransparent = true
        window.titleVisibility = .hidden
        window.contentView = canvas
        window.makeKeyAndOrderFront(nil)
        window.makeFirstResponder(canvas)
        NSApp.activate(ignoringOtherApps: true)
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ app: NSApplication) -> Bool { true }
}

let app = NSApplication.shared
let delegate = App()
app.delegate = delegate
app.setActivationPolicy(.regular)
app.run()
