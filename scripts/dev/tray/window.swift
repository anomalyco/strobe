// A tiny AppKit app for strobe's windowless guards (dev only, macOS): it makes a 480×320 window and
// either keeps it hidden (`hidden`: never ordered in, as a tray app's settings window is until asked
// for) or shows it (`shown`). It prints `ready` once the window exists, then ticks every 50 ms.
// Build: swiftc -O -o scripts/dev/tray/window scripts/dev/tray/window.swift
import AppKit

let mode = CommandLine.arguments.dropFirst().first ?? "hidden"
let app = NSApplication.shared
app.setActivationPolicy(mode == "shown" ? .regular : .accessory)

final class Delegate: NSObject, NSApplicationDelegate {
    var window: NSWindow?
    var ticks = 0
    func applicationDidFinishLaunching(_ note: Notification) {
        let w = NSWindow(contentRect: NSRect(x: 200, y: 200, width: 480, height: 320), styleMask: [.titled, .closable], backing: .buffered, defer: false)
        w.title = "strobe window fixture"
        w.isReleasedWhenClosed = false
        if mode == "shown" {
            w.makeKeyAndOrderFront(nil)
            NSApp.activate(ignoringOtherApps: true)
        }
        window = w
        print("ready \(mode)")
        fflush(stdout)
        Timer.scheduledTimer(withTimeInterval: 0.05, repeats: true) { _ in self.ticks += 1 }
    }
}

let delegate = Delegate()
app.delegate = delegate
app.run()
