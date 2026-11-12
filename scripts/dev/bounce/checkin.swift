// Known-answer fixture for the macOS check-in watch (`host::checkin`; dev only, not shipped): an app
// bundle that checks in with Launch Services (AppKit's `finishLaunching` inside `-[NSApp run]`, which
// stops the Dock's bounce) after a delay given on its command line, and shows no window. Built on a Mac:
//   mkdir -p Checkin.app/Contents/MacOS
//   swiftc -O -o Checkin.app/Contents/MacOS/checkin scripts/dev/bounce/checkin.swift
//   cp scripts/dev/bounce/Checkin.plist Checkin.app/Contents/Info.plist
//   codesign -s - Checkin.app
//
//   checkin exit                exits at once, before AppKit runs
//   (as `stub`)                 a bundle's launcher stub: when this binary runs under the name `stub`, it
//                               re-executes its sibling `checkin` in the same folder with the same
//                               arguments and environment (same pid), as some apps' launchers do
//   checkin <ms> <marks file>   sleeps <ms> from `main`, then runs the app (`-[NSApp run]`); writes
//                               `main_ns`, `run_ns` (just before `run`), `will_ns` and `did_ns` (its
//                               will- and did-finish-launching callbacks), mach time in ns (the clock of
//                               strobe's `host::now_ns`), to <marks file>, then quits after 10 s
//
// Calling `finishLaunching` without `run` never checks in (Launch Services' `isFinishedLaunching` stayed
// off for 3 s), so the fixture runs the app as `NSApplicationMain` does.
import AppKit
import Darwin

var timebase = mach_timebase_info_data_t()
mach_timebase_info(&timebase)
func now() -> UInt64 { mach_absolute_time() * UInt64(timebase.numer) / UInt64(timebase.denom) }

let startedAt = now()
let args = CommandLine.arguments
// A launcher stub: the real binary takes over this process (exec keeps the pid and the environment).
if URL(fileURLWithPath: args[0]).lastPathComponent == "stub" {
    let real = URL(fileURLWithPath: args[0]).deletingLastPathComponent().appendingPathComponent("checkin").path
    var argv: [UnsafeMutablePointer<CChar>?] = ([real] + args.dropFirst()).map { strdup($0) }
    argv.append(nil)
    execv(real, argv)
    exit(1)
}
// `checkin exit`: an app that exits at once, before AppKit runs (strobe must signal nothing by its pid).
if args.count > 1 && args[1] == "exit" { exit(0) }
let delay = args.count > 1 ? (UInt32(args[1]) ?? 200) : 200
let marks = args.count > 2 ? args[2] : "/dev/null"
var runAt: UInt64 = 0

final class Delegate: NSObject, NSApplicationDelegate {
    var willAt: UInt64 = 0
    func applicationWillFinishLaunching(_ note: Notification) { willAt = now() }
    func applicationDidFinishLaunching(_ note: Notification) {
        let didAt = now()
        try? "main_ns=\(startedAt)\nrun_ns=\(runAt)\nwill_ns=\(willAt)\ndid_ns=\(didAt)\n".write(toFile: marks, atomically: true, encoding: .utf8)
        DispatchQueue.main.asyncAfter(deadline: .now() + 10) { NSApp.terminate(nil) }
    }
}

let app = NSApplication.shared
let delegate = Delegate()
app.delegate = delegate
_ = app.setActivationPolicy(.regular)
usleep(delay * 1000)
runAt = now()
app.run()
