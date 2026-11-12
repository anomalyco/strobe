// Private names marked where a message is composed (src/text.rs; SPEC 1 rule 9), checked by the compiler and on real
// strings (dev, not shipped). It builds a small crate against strobe's library in <target>/private-text and:
//
//   compile-fail  each snippet below must fail to compile with its error: a `Private` formatted with `{}`, `{:?}` or
//                 `{p}`, turned into a string, read through `expose_local()` from outside, or given to `msg!`; a path's
//                 `display()` given to `msg!` (any delimiter); a `{:?}` argument, a captured `{x}` (an `Error`, a
//                 `Text`, a number) or a width from an argument in `msg!`; a `Text` formatted with `{}`;
//   compiles      the control: every kind of argument `msg!` takes, and `Error`/`Refusal` called as every caller
//                 calls them today (plain strings, `impl Into<String>`), compile;
//   examples      each constructor on real strings (other processes and their shares, a covering window's title,
//                 covering apps, a phone, a CPU reservation, paths in the repository, the state folder, the home
//                 folder and elsewhere, URLs, a tool's output), nested in messages with multi-byte text before the
//                 marks, cut short, padded, through an `Error`, a `Refusal`, its `..r` copies and result.json: each
//                 local rendering is today's text, each public one the expected words, counts and process ids kept,
//                 no private part left; and every input a review found that once leaked (marks kept beside an
//                 edited field, `..` in a path, URL and output forms), each of which must hide what it names;
//   canary        the same binary under STROBE_DEV_CANARY=1: every marked part's local text tagged, no public text.
//
// The crate builds offline (strobe's own dependencies, already fetched) and its sources are removed after; its
// compiled dependencies stay warm in the target folder.
//
//   bun scripts/dev/private-text.ts [--show]
//
// Exit 1 when a snippet compiled that must not, the control failed, or an example differs.
import { copyFileSync, mkdirSync, rmSync, writeFileSync } from "node:fs"
import { join } from "node:path"

const root = join(import.meta.dir, "../..")
const target = process.env.CARGO_TARGET_DIR ?? join(root, "target")
const dir = join(target, "private-text")
const show = process.argv.includes("--show")

// name → [the snippet's body, the error code it must fail with, words its message must hold]
const fails: Record<string, [string, string, string]> = {
  display: [`fn f(p: &Private<String>) -> String { format!("{}", p) }`, "E0277", "Display"],
  debug: [`fn f(p: &Private<String>) -> String { format!("{:?}", p) }`, "E0277", "Debug"],
  inline: [`fn f(p: &Private<String>) -> String { format!("{p}") }`, "E0277", "Display"],
  to_string: [`fn f(p: &Private<String>) -> String { p.to_string() }`, "E0599", "to_string"],
  expose: [`fn f(p: &Private<String>) -> String { p.expose_local().clone() }`, "E0624", "private"],
  msg_private: [`fn f(p: &Private<String>) -> Text { strobe::msg!("covered by {}", p) }`, "E0277", "cannot go into a message"],
  msg_path: [`fn f(p: &std::path::Path) -> Text { strobe::msg!("in {}", p.display()) }`, "E0277", "cannot go into a message"],
  msg_debug: [`fn f(t: &Text) -> Text { strobe::msg!("{:?}", t) }`, "E0277", "Debug"],
  text_display: [`fn f(t: &Text) -> String { format!("{}", t) }`, "E0277", "Display"],
  // A capture would format an Error or a Text through Display or Debug and drop its marks.
  msg_capture_error: [`fn f(e: &Error) -> Text { strobe::msg!("stopped: {e}") }`, "E0080", "positional"],
  msg_capture_debug: [`fn f(t: &Text) -> Text { strobe::msg!("{t:?}") }`, "E0080", "positional"],
  msg_capture_unicode: [`fn f(α: &Error) -> Text { strobe::msg!("stopped: {α}") }`, "E0080", "positional"],
  msg_capture_number: [`fn f(n: u32) -> Text { strobe::msg!("{n} blocks") }`, "E0080", "positional"],
  msg_capture_width: [`fn f(w: usize) -> Text { strobe::msg!("{:w$}", "a") }`, "E0080", "positional"],
  msg_brackets: [`fn f(p: &std::path::Path) -> Text { strobe::msg!["in {}", p.display()] }`, "E0277", "cannot go into a message"],
}

const control = `
fn plain(s: impl Into<String>) -> Error { Error::msg(s) }
fn f(t: &Text, e: &Error, r: &Refusal, p: &Private<String>, n: u32, x: f64, status: std::process::ExitStatus) -> Vec<Text> {
    let s = String::from("s");
    let cow: std::borrow::Cow<str> = "c".into();
    let pid = 4120;
    let _ = Error::msg("a str");
    let _ = Error::msg(format!("a String {n}"));
    let _ = Error::usage(&s);
    let _ = Error::new(1, cow.clone()).hint("a hint");
    let _ = Error::refused("Covered", format!("{x:.1}"), &s);
    let _ = plain("generic");
    let _ = Refusal::new(ValidatorId::Covered, "why").detail(&s).fix(String::new());
    let _ = Refusal { summary: format!("on head: {}", r.summary), ..r.clone() };
    let _ = Error { message: format!("launch 1 of 2: {}", e.message), ..e.clone() };
    vec![
        strobe::msg!("plain"),
        strobe::msg!("{} {} {} {} {:>6.2} {}", "str", s, cow, n, x, 'c'),
        strobe::msg!("{0} {1} {0} {2:.0} {{literal}} {3:>8} {3:<2.1}", n, 8, x, "w"),
        strobe::msg!("{}; {}; {}; {}", t, e, r, ValidatorId::Covered),
        strobe::msg!("{} of process {}", p.quoted_or("a window"), pid),
        strobe::msg!("adb exited {}", status),
        strobe::msg!("{}", Text::device(p, false)),
        strobe::msg!("{}", Text::busiest_processes("busiest: ", true, &[(p.clone(), 1.0)], None)),
        strobe::msg!("{}", p.redacted("the phone")),
        strobe::msg!("{}", Text::path(std::path::Path::new("x"), None)),
    ]
}
`

const examples = `
use strobe::host;
use std::path::{Path, PathBuf};

struct Check { failed: usize, show: bool }

impl Check {
    /// One example: its local text as today's code writes it, its public text, and the private parts that
    /// must sit at the marked bytes and never show in public.
    fn text(&mut self, what: &str, t: &Text, local: &str, public: &str, private: &[&str]) {
        let mut why = Vec::new();
        if t.local() != local { why.push(format!("local {:?}, expected {:?}", t.local(), local)); }
        if t.public() != public { why.push(format!("public {:?}, expected {:?}", t.public(), public)); }
        for s in t.spans() {
            let ok = s.at[0] <= s.at[1] && s.at[1] <= t.local().len() && t.local().is_char_boundary(s.at[0]) && t.local().is_char_boundary(s.at[1]);
            if !ok { why.push(format!("mark {:?} is not on the text's characters", s.at)); }
        }
        let marked: Vec<&str> = t.spans().iter().filter(|s| s.at[1] <= t.local().len()).map(|s| &t.local()[s.at[0]..s.at[1]]).collect();
        for p in private {
            if t.public().contains(p) { why.push(format!("public shows {p:?}")); }
            if !marked.iter().any(|m| m.contains(p)) { why.push(format!("{p:?} is in no mark ({marked:?})")); }
        }
        let bad = !why.is_empty();
        self.failed += bad as usize;
        if bad || self.show {
            println!("{} {what}\\n  local:  {}\\n  public: {}", if bad { "FAIL" } else { "ok  " }, t.local(), t.public());
            for w in why { println!("  {w}"); }
        } else {
            println!("ok   {what}");
        }
    }
}

impl Check {
    /// A text whose public form must hold none of \`private\` (case aside), and read \`public\` when one is given.
    fn hides(&mut self, what: &str, t: &Text, public: Option<&str>, private: &[&str]) {
        let shown = t.public().to_lowercase();
        let mut why: Vec<String> = private.iter().filter(|p| shown.contains(&p.to_lowercase())).map(|p| format!("public shows {p:?}")).collect();
        if let Some(p) = public { if t.public() != p { why.push(format!("public {:?}, expected {:?}", t.public(), p)); } }
        let bad = !why.is_empty();
        self.failed += bad as usize;
        if bad || self.show {
            println!("{} {what}\\n  local:  {:?}\\n  public: {:?}", if bad { "FAIL" } else { "ok  " }, t.local(), t.public());
            for w in why { println!("  {w}"); }
        } else {
            println!("ok   {what}");
        }
    }
}

fn private(s: &str, kind: PrivKind) -> Private<String> { Private::new(s.to_string(), kind) }

/// Each helper in the sentence it is for, its format string copied from the code on main (file:line at
/// 0.31.0): local exactly what that code writes today, public a sentence that reads right aloud.
fn sentences(c: &mut Check) {
    use std::collections::BTreeMap;
    let fp = PrivKind::ForeignProc;
    // The phone's short UDID (host/ios.rs, target/wir.rs): "the phone ({})" keeps "the phone", drops the id.
    let raw = "00008110-001A2B3C4D5E6F70";
    let short = private(raw, PrivKind::DeviceId).tail_chars(8);
    let id = strobe::host::ios::short_udid(raw);
    let tail = "plug it in and tap Trust, then enter its passcode; strobe never pairs by itself";
    // src/host/ios.rs:351  "unlock the phone ({}), plug it in and tap Trust, then enter its passcode; strobe never pairs by itself"
    c.text("ios.rs:351", &msg!("unlock the phone{}, {}", Text::phone_id(&short), tail), &format!("unlock the phone ({id}), {tail}"), &format!("unlock the phone, {tail}"), &[id.as_str()]);
    // src/host/ios.rs:858  "the phone ({}) is locked"
    c.text("ios.rs:858", &msg!("the phone{} is locked", Text::phone_id(&short)), &format!("the phone ({id}) is locked"), "the phone is locked", &[id.as_str()]);
    // src/host/ios.rs:915  "the phone ({}) is locked, so Safari and Web Inspector will not expose tabs"
    c.text(
        "ios.rs:915",
        &msg!("the phone{} is locked, so Safari and Web Inspector will not expose tabs", Text::phone_id(&short)),
        &format!("the phone ({id}) is locked, so Safari and Web Inspector will not expose tabs"),
        "the phone is locked, so Safari and Web Inspector will not expose tabs",
        &[id.as_str()],
    );
    // src/target/wir.rs:989  "lost connection to the phone ({short_udid}): {why}"
    let why = "the inspector socket closed";
    c.text("wir.rs:989", &msg!("lost connection to the phone{}: {}", Text::phone_id(&short), why), &format!("lost connection to the phone ({id}): {why}"), &format!("lost connection to the phone: {why}"), &[id.as_str()]);
    // An adb serial named alone (validity.rs).
    let serial = private("R5CT20ABCDE", PrivKind::DeviceId);
    let s = "R5CT20ABCDE";
    // src/validity.rs:787  "{serial} is hot {when}: {why}"
    let (when, why) = ("before block 3", "its battery is at 44 °C");
    c.text("validity.rs:787", &msg!("{} is hot {}: {}", Text::device(&serial, false), when, why), &format!("{s} is hot {when}: {why}"), &format!("the phone is hot {when}: {why}"), &[s]);
    // src/validity.rs:798  "{} is an emulator ({why}): it runs on this computer's CPU and GPU, not a phone's, so its timing describes no phone"
    let (emu, e) = (private("emulator-5554", PrivKind::DeviceId), "emulator-5554");
    let rest = "it runs on this computer's CPU and GPU, not a phone's, so its timing describes no phone";
    c.text("validity.rs:798", &msg!("{} is an emulator ({}): {}", emu.redacted("the device"), "ro.kernel.qemu is 1", rest), &format!("{e} is an emulator (ro.kernel.qemu is 1): {rest}"), &format!("the device is an emulator (ro.kernel.qemu is 1): {rest}"), &[e]);
    // src/validity.rs:805  "{} is reached over the network (adb over Wi-Fi): the radio's work and the link's latency would be in the numbers"
    let wifi = private("192.168.1.23:5555", PrivKind::DeviceId);
    let rest = "is reached over the network (adb over Wi-Fi): the radio's work and the link's latency would be in the numbers";
    c.text("validity.rs:805", &msg!("{} {}", Text::device(&wifi, false), rest), &format!("192.168.1.23:5555 {rest}"), &format!("the phone {rest}"), &["192.168"]);
    // src/validity.rs:811  "{} has Battery Saver on: it slows the CPU and GPU and defers work"
    let rest = "has Battery Saver on: it slows the CPU and GPU and defers work";
    c.text("validity.rs:811", &msg!("{} {}", Text::device(&serial, false), rest), &format!("{s} {rest}"), &format!("the phone {rest}"), &[s]);
    // src/validity.rs:814  "{}'s battery is at {}%: phones slow down under {LOW_BATTERY_PCT}% to make it last" (20)
    c.text(
        "validity.rs:814",
        &msg!("{}'s battery is at {}%: phones slow down under {}% to make it last", Text::device(&serial, false), 12, 20),
        &format!("{s}'s battery is at 12%: phones slow down under 20% to make it last"),
        "the phone's battery is at 12%: phones slow down under 20% to make it last",
        &[s],
    );

    // Other processes (validity.rs, bench.rs): named_work with its lead-in.
    let who = [(private("tracker (beta).exe (4120, other user)", fp), 421.4), (private("renderfarm.exe (88)", fp), 273.0)];
    let plain: Vec<(String, f64)> = vec![("tracker (beta).exe (4120, other user)".into(), 421.4), ("renderfarm.exe (88)".into(), 273.0)];
    let named = strobe::validity::named_work(&plain, None);
    let names = ["tracker", "renderfarm"];
    // src/validity.rs:640  "busiest processes (% of a core): {}"
    c.text(
        "validity.rs:640",
        &Text::busiest_processes("busiest processes (% of a core): ", false, &who, None),
        &format!("busiest processes (% of a core): {named}"),
        "2 other processes, the busiest 421% of a core",
        &names,
    );
    // src/app/bench.rs:6039  "busiest processes on the machine (% of a core): {named}{again}" (again: bench.rs:8109)
    let again = "; slow twice, the second time after the CPUs settled (the first: Throttled: the speed check ran 1.42x slower)";
    c.text(
        "bench.rs:6039",
        &msg!("{}{}", Text::busiest_processes("busiest processes on the machine (% of a core): ", true, &who, None), again),
        &format!("busiest processes on the machine (% of a core): {named}{again}"),
        &format!("2 other processes on the machine, the busiest 421% of a core{again}"),
        &names,
    );
    // src/app/bench.rs:6043  "other work on the target's CPUs meanwhile: {}; busiest processes on the machine (% of a core): {named}{again}"
    let on = "88% of a core";
    c.text(
        "bench.rs:6043",
        &msg!("other work on the target's CPUs meanwhile: {}; {}{}", on, Text::busiest_processes("busiest processes on the machine (% of a core): ", true, &who, None), again),
        &format!("other work on the target's CPUs meanwhile: {on}; busiest processes on the machine (% of a core): {named}{again}"),
        &format!("other work on the target's CPUs meanwhile: {on}; 2 other processes on the machine, the busiest 421% of a core{again}"),
        &names,
    );
    // src/app/bench.rs:8093  "{}; other work on the target's CPUs {}; busiest processes on the machine: {}"
    let err = Error::refused("Throttled", "the speed check ran 1.42x slower than this device's reference", "x");
    c.text(
        "bench.rs:8093",
        &msg!("{}; other work on the target's CPUs {}; {}", err, on, Text::busiest_processes("busiest processes on the machine: ", true, &who, None)),
        &format!("{}; other work on the target's CPUs {on}; busiest processes on the machine: {named}", err.message),
        &format!("{}; other work on the target's CPUs {on}; 2 other processes on the machine, the busiest 421% of a core", err.message),
        &names,
    );
    // src/app/bench.rs:1637 and 1656  "other processes of this user kept off CPUs {cpus} during {} timed block{}: {:.0} moved per block ({names}{})" … "; {} could not be moved: {failed}"
    let mut moved = BTreeMap::new();
    for (n, k) in [("indexer", 3u32), ("chat-helper", 2), ("backup", 1)] {
        moved.insert(private(n, fp), k);
    }
    let mut failed = BTreeMap::new();
    failed.insert(private("guard.exe", fp), "elevated".to_string());
    let mut t = msg!("other processes of this user kept off CPUs {} during {} timed block{}: {:.0} moved per block{}", "16-31", 2, "s", 2.0, Text::reserve_moved(&moved));
    t.push(&msg!("; {}", Text::reserve_failed(&failed)));
    c.text(
        "bench.rs:1637+1656",
        &t,
        "other processes of this user kept off CPUs 16-31 during 2 timed blocks: 2 moved per block (indexer ×3, chat-helper ×2, backup); 1 could not be moved: guard.exe (elevated)",
        "other processes of this user kept off CPUs 16-31 during 2 timed blocks: 2 moved per block; 1 could not be moved (elevated)",
        &["indexer", "chat-helper", "backup", "guard"],
    );
    let t = msg!("other processes of this user kept off CPUs {} during {} timed block{}: {:.0} moved per block{}", "16-31", 1, "", 0.0, Text::reserve_moved(&BTreeMap::new()));
    c.text("bench.rs:1637, none moved", &t, "other processes of this user kept off CPUs 16-31 during 1 timed block: 0 moved per block ()", "other processes of this user kept off CPUs 16-31 during 1 timed block: 0 moved per block ()", &[]);

    // Windows in front of the target's.
    let covered = "window is covered by other apps' windows (strobe brings it to the front only after a minute without keyboard or mouse input, and with the Accessibility permission)";
    let apps = [private("Mail Pro", fp), private("Notes+", fp)];
    // src/target/native.rs:211 and 222  "; in front of it: {}" in "the app's {why}{by}"
    c.text(
        "native.rs:211+222",
        &msg!("the app's {}{}", covered, msg!("; in front of it: {}", Text::covering_apps(&apps))),
        &format!("the app's {covered}; in front of it: Mail Pro, Notes+"),
        &format!("the app's {covered}; in front of it: 2 other apps"),
        &["Mail Pro", "Notes+"],
    );
    let title = private("Plan — Q3", PrivKind::WindowTitle);
    // src/target/a11y/input.rs:579 (and wl_input.rs:233, xtest.rs:140)  "another window covers the point ({x}, {y}): {:?} of process {pid}"
    c.text(
        "input.rs:579",
        &msg!("another window covers the point ({}, {}): {} of process {}", 10, 20, title.quoted_or("a window"), 4120),
        "another window covers the point (10, 20): \\"Plan — Q3\\" of process 4120",
        "another window covers the point (10, 20): a window of process 4120",
        &["Plan"],
    );
    // src/target/a11y/input.rs:595  "a {:?} window of process {pid}"
    let class = private("NorthwindFrame", PrivKind::WindowTitle);
    c.text(
        "input.rs:595",
        &msg!("{} of process {}", Text::marked(msg!("a {} window", class.quoted_or("")), PrivKind::WindowTitle, "a window"), 4120),
        "a \\"NorthwindFrame\\" window of process 4120",
        "a window of process 4120",
        &["Northwind"],
    );
    // src/target/a11y/cg_input.rs:291 and 297  "another app's window covers the point ({x:.0}, {y:.0}): {what} of process {pid}", what = "a window of {owner}"
    let owner = private("Mail Pro", fp);
    c.text(
        "cg_input.rs:291+297",
        &msg!("another app's window covers the point ({:.0}, {:.0}): {} of process {}", 10.4, 20.6, Text::marked(msg!("a window of {}", owner.redacted("")), fp, "a window"), 4120),
        "another app's window covers the point (10, 21): a window of Mail Pro of process 4120",
        "another app's window covers the point (10, 21): a window of process 4120",
        &["Mail Pro"],
    );
    // src/host/x11.rs:378 and 379  "{:?} (pid {})" in "the window is not on top at ({cx}, {cy}): {over} is, and the X server copies what covers a window"
    c.text(
        "x11.rs:378+379",
        &msg!("the window is not on top at ({}, {}): {} (pid {}) is, and the X server copies what covers a window", 640, 360, Text::window(&title), 4120),
        "the window is not on top at (640, 360): \\"Plan — Q3\\" (pid 4120) is, and the X server copies what covers a window",
        "the window is not on top at (640, 360): another window (pid 4120) is, and the X server copies what covers a window",
        &["Plan"],
    );
    // src/host/wayland.rs:442  "{:?} of process {} covers the region"
    c.text("wayland.rs:442", &msg!("{} of process {} covers the region", title.quoted_or("a window"), 4120), "\\"Plan — Q3\\" of process 4120 covers the region", "a window of process 4120 covers the region", &["Plan"]);
}

/// The second review's inputs (cleanup/priv1 f23e91d): each must hide what it names.
fn review2(c: &mut Check) {
    let home = host::home().display().to_string();
    // 1. Tool output.
    for (raw, private) in [
        ("GET https://example.test/app;jsessionid=s3cr3t?v=1;token=t0k3n failed", &["s3cr3t", "t0k3n"][..]),
        ("fetch https://example.test/api?fields=a,b&token=s3cr3t failed", &["s3cr3t"][..]),
        ("connect http://[::1]:8080/api?token=s3cr3t failed", &["s3cr3t"][..]),
        ("GET https://example.test/api?filter[token]=s3cr3t", &["s3cr3t"][..]),
        ("at /Users/ann/a.ts https://ann:hunter2@example.test?token=s3cr3t", &["hunter2", "s3cr3t", "/ann"][..]),
        ("\\x1b[36m\\x1b(B/Users/ann/northwind/a.ts\\x1b[0m:3:1", &["ann", "northwind"][..]),
        ("\\u{9b}36m/Users/ann/northwind/a.ts\\u{9b}0m:3:1", &["ann", "northwind"][..]),
    ] {
        c.hides(&format!("output2: {raw:?}"), &Text::external_output(raw, None), None, private);
    }
    c.hides("output2: a drive's C://", &Text::external_output("open C://Users/ann/northwind/a.txt", None), Some("open ~/…/a.txt"), &["ann", "northwind"]);
    c.hides("output2: a :// no URL rule reads", &Text::external_output("see x@https://ann:pw7@example.test/", None), Some("see [redacted]"), &["pw7"]);
    c.hides("output2: a newline after a one-part path", &Text::external_output("120 /s\\nsrc/a.ts:3:1: error TS2322", None), Some("120 /s\\nsrc/a.ts:3:1: error TS2322"), &[]);
    let user = Path::new(&home).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    c.hides("output2: the home folder before a colon", &Text::external_output(&format!("cannot open {home}: Permission denied"), None), Some("cannot open ~: Permission denied"), &[&user]);
    c.hides("output2: the home folder in PATH", &Text::external_output(&format!("PATH=/usr/bin:{home}"), None), Some("PATH=…/bin:~"), &[&user]);
    c.hides("output2: the home folder before a full stop", &Text::external_output(&format!("cannot find {home}."), None), Some("cannot find ~."), &[&user]);
    // 2. Another user's home folder.
    use strobe::text::public_path_in as at;
    for (path, home, public) in [
        ("/Users/SECRET28", "/Users/ann", "~"),
        ("/Users/SECRET28/a.ts", "/Users/ann", "~/a.ts"),
        ("/home/SECRET21/src/x/a.ts", "/home/ann", "~/…/a.ts"),
        ("/mnt/c/Users/Alice", "/home/ubuntu", "~"),
        ("/var/home/SECRET22/x", "/home/ann", "~/x"),
        (r"C:\\Users\\SECRET19\\x\\a.ts", r"C:\\Users\\ann", "~/…/a.ts"),
        (r"\\\\srv\\Users\\SECRET23\\a.ts", r"C:\\Users\\ann", "~/a.ts"),
    ] {
        let got = at(path, None, "/nowhere/strobe", home);
        let ok = got == public;
        c.failed += !ok as usize;
        println!("{} path2 {path:?}: {got:?}{}", if ok { "ok  " } else { "FAIL" }, if ok { String::new() } else { format!(", expected {public:?}") });
    }
    c.hides("output2: other users' homes", &Text::external_output(r"in C:\\Users\\SECRET19; /home/SECRET21", None), None, &["SECRET19", "SECRET21"]);
    // 3. URLs.
    for (url, private) in [
        ("file:///Users/ann/northwind/index.html?token=s3cr3t", &["s3cr3t", "northwind"][..]),
        ("file:///Users/ann/northwind/index.html#/login#token=s3cr3t", &["s3cr3t", "northwind"][..]),
        ("https://example.test/api?filter[token]=s3cr3t&auth[key]=k3y", &["s3cr3t", "k3y"][..]),
        ("mailto:ann@northwind-secret.com?subject=hi&token=t0k", &["ann", "northwind", "t0k"][..]),
    ] {
        c.hides(&format!("url2: {url}"), &Text::url(url), None, private);
    }
    c.hides("url2: an @ in a slashless query stays", &Text::url("https://example.test?user=a@b.com&v=1"), Some("https://example.test?user=a@b.com&v=1"), &[]);
    let long = format!("https://example.test/?x={}", "=".repeat(200_000));
    let t0 = std::time::Instant::now();
    let _ = Text::url(&long);
    let ms = t0.elapsed().as_millis();
    c.failed += (ms > 2000) as usize;
    println!("{} url2: 200 000 '=' scrubbed in {ms} ms", if ms > 2000 { "FAIL" } else { "ok  " });
    // 5. The bridge: two marked fields swapped or combined; equality.
    let dev = private("R5CT20ABCDE", PrivKind::DeviceId);
    let other = private("SecretApp.exe", PrivKind::ForeignProc);
    let r = Refusal::new(ValidatorId::Thermal, msg!("{} is hot", dev.redacted("the phone"))).detail(msg!("{} is hot; {}", dev.redacted("the phone"), other.redacted("1 other process")));
    let swapped = Refusal { summary: r.detail.clone().unwrap_or_default(), detail: Some(r.summary.clone()), ..r.clone() };
    c.hides("bridge: fields swapped", &swapped.summary_text(), None, &["SecretApp", "R5CT20ABCDE"]);
    c.hides("bridge: fields swapped (the other)", &swapped.detail_text().unwrap_or_default(), None, &["SecretApp", "R5CT20ABCDE"]);
    let joined = Refusal { summary: format!("{} ({})", r.summary, r.detail.clone().unwrap_or_default()), ..r.clone() };
    c.hides("bridge: fields combined", &joined.summary_text(), None, &["SecretApp", "R5CT20ABCDE"]);
    let plain = Refusal::new(ValidatorId::Thermal, "plain").detail(other.redacted("1 other process"));
    let copied = Refusal { summary: format!("plain: {}", plain.detail.clone().unwrap_or_default()), ..plain.clone() };
    c.hides("bridge: an unmarked field given another's private part", &copied.summary_text(), None, &["SecretApp"]);
    let a = Refusal::new(ValidatorId::Thermal, "s").detail(dev.redacted("the phone")).fix(other.redacted("x"));
    let b = Refusal::new(ValidatorId::Thermal, "s").fix(other.redacted("x")).detail(dev.redacted("the phone"));
    let ok = a == b;
    c.failed += !ok as usize;
    println!("{} bridge: equal whatever order detail and fix were set", if ok { "ok  " } else { "FAIL" });
    let r2 = Refusal { summary: format!("on head: {}", r.summary), ..r.clone() };
    let back: Refusal = serde_json::from_str(&serde_json::to_string(&r2).expect("json")).expect("read back");
    let ok = back == r2;
    c.failed += !ok as usize;
    println!("{} bridge: a ..r copy equals itself read back", if ok { "ok  " } else { "FAIL" });
    // 6. The reserve list as bench writes it (× and an empty list unmarked).
    let mut moved = std::collections::BTreeMap::new();
    moved.insert(private("indexer", PrivKind::ForeignProc), 3u32);
    c.text("reserve: × as bench writes it", &Text::reserve_moved(&moved), " (indexer ×3)", "", &["indexer"]);
    let none = Text::reserve_moved(&std::collections::BTreeMap::new());
    if none.is_marked() || none.local() != " ()" { c.failed += 1; println!("FAIL reserve: an empty list is marked or changed: {:?}", none.local()); } else { println!("ok   reserve: an empty list is unmarked"); }
}

/// Planted secrets combined with the separators, brackets, escapes and wrappers both reviews listed: none may
/// survive any scrubber; a fixed list of harmless strings passes through unchanged.
fn fuzz(c: &mut Check) {
    let (u, f, t, p) = ("zqxuser", "zqxfolder", "zqxtoken", "zqxpass");
    let home = host::home().display().to_string();
    let paths: Vec<String> = [
        "/Users/{U}", "/Users/{U}/a.ts", "/home/{U}/src/a.ts", "/var/home/{U}/x", "/mnt/c/Users/{U}/x/a.ts", r"C:\\Users\\{U}\\a.ts", "C:/Users/{U}/a.ts",
        "C://Users/{U}/{F}/a.txt", r"\\\\srv\\Users\\{U}\\a.ts", "//srv/Users/{U}/a.ts", "/opt/{F}/bin/tool", "/Volumes/{F}/src/a.ts", r"D:\\{F}\\x\\a.ts", "/srv/{F}/app/a.ts",
        "/mnt/{F}/x/a.ts", "~/{F}/a.ts", "/tmp/{F}/../{F}/a.ts", "/root/{F}/a.ts", "{H}/{F}/a.ts", "{H}", "/private/var/{F}/x/a.ts", r"\\\\?\\C:\\{F}\\x\\a.ts",
    ]
    .iter()
    .map(|s| s.replace("{U}", u).replace("{F}", f).replace("{H}", &home))
    .collect();
    let urls: Vec<String> = [
        "https://example.test/a?token={T}", "https://example.test/a;jsessionid={T}?v=1", "https://example.test/#/login#token={T}", "https://example.test/api?fields=a,b&token={T}",
        "https://example.test/api?filter[token]={T}", "http://[::1]:8080/api?token={T}", "https://u:{P}@example.test/x", "https://u:{P}#x@example.test/x", "https://u:{P}?x@example.test/x",
        "ws://u:{P}@localhost:9222/x", "https://example.test/a?accessToken={T}&v=1", "https://example.test/a?X-Amz-Signature={T}", "file:///Users/{U}/x/index.html?token={T}",
        "file:///C:/Users/{U}/x/a.html#token={T}", "mailto:{U}@example.test", "data:text/html,{F}", "HTTPS://U:{P}@EXAMPLE.TEST/?TOKEN={T}",
    ]
    .iter()
    .map(|s| s.replace("{U}", u).replace("{F}", f).replace("{T}", t).replace("{P}", p))
    .collect();
    let wraps: &[(&str, &str)] = &[
        ("", ""), ("\\"", "\\""), ("'", "'"), ("(", ")"), ("[", "]"), ("<", ">"), ("\`", "\`"), ("{", "}"), ("=", ""), ("--out=", ""), ("PATH=/usr/bin:", ":/bin"), ("", ":3:1"),
        ("", ":3:1: error TS2322"), ("", "(3,1)"), ("", "."), ("", ","), ("", ";"), ("", ":"), ("\\x1b[36m", "\\x1b[0m"), ("\\x1b(B", ""), ("\\u{9b}31m", "\\u{9b}0m"), ("\\x1b]8;;", "\\x07"),
        ("\\u{9d}0;t\\u{9c}", ""), ("", " https://example.test/?q=1"), ("x ", " y"), ("\\x1b[1m\\x1b(B", "\\x1b[m"),
    ];
    let lines: &[(&str, &str)] = &[("", ""), ("error: ", ""), ("at ", "\\nnext line"), ("\\t", "\\r\\n"), ("120 /s\\n", ""), ("open ", " failed")];
    let secrets = [u, f, t, p];
    let (mut n, mut bad) = (0, Vec::new());
    for x in paths.iter().chain(urls.iter()) {
        for (a, b) in wraps {
            for (l, r) in lines {
                let raw = format!("{l}{a}{x}{b}{r}");
                let public = Text::external_output(&raw, None).public().to_lowercase();
                n += 1;
                if secrets.iter().any(|s| public.contains(s)) { bad.push((raw, public)); }
            }
        }
    }
    for x in &paths {
        let public = Text::path(Path::new(x), None).public().to_lowercase();
        n += 1;
        if secrets.iter().any(|s| public.contains(s)) { bad.push((x.clone(), public)); }
    }
    for x in &urls {
        for (a, b) in [("", ""), ("", "&v=1"), ("", "#top")] {
            let raw = format!("{a}{x}{b}");
            let public = Text::url(&raw).public().to_lowercase();
            n += 1;
            if secrets.iter().any(|s| public.contains(s)) { bad.push((raw, public)); }
        }
    }
    c.failed += !bad.is_empty() as usize;
    println!("{} fuzz: {n} inputs, {} kept a planted secret", if bad.is_empty() { "ok  " } else { "FAIL" }, bad.len());
    for (raw, public) in bad.iter().take(12) { println!("  {raw:?}\\n    -> {public:?}"); }
    let harmless = [
        "/s", "3/4", "n/a", "and/or", "2026-11-12", "2026/11/12", "v1.2.3", "0.31.0", "src/a.ts:3:1", "?v=1&page=2", "120 /s, 3/4 done, 10 ms/run", "a/b/c",
        "src/a.ts:3:1: error TS2322: Type 'x' is not assignable", "see https://example.test/docs?v=1&page=2", "1/2\\n3/4\\r\\n", "C:", "x = y; z: w", "(see a/b)",
    ];
    let changed: Vec<_> = harmless.iter().filter(|h| Text::external_output(h, None).public() != **h).collect();
    c.failed += !changed.is_empty() as usize;
    println!("{} fuzz: {} harmless strings unchanged{}", if changed.is_empty() { "ok  " } else { "FAIL" }, harmless.len(), if changed.is_empty() { String::new() } else { format!(", changed: {changed:?}") });
    for (url, what) in [("https://example.test/app?v=1&page=2", "a plain query"), ("https://example.test/a?tokenize=1&keyboard=us&author=x", "lookalike keys")] {
        c.hides(&format!("fuzz: {what} stays"), &Text::url(url), Some(url), &[]);
    }
}

/// What the next packages build on (the review's Item 11), each on real strings.
fn api(c: &mut Check) {
    use std::collections::BTreeMap;
    let fp = PrivKind::ForeignProc;
    let who = [(private("tracker (beta).exe", fp), 421.4), (private("renderfarm.exe", fp), 273.0)];
    let plain: Vec<(String, f64)> = vec![("tracker (beta).exe".into(), 421.4), ("renderfarm.exe".into(), 273.0)];
    let names = ["tracker", "renderfarm"];
    // validity::named_work with its lead-in: local exactly named_work's, public one sentence.
    let lead = "busiest processes on the machine (% of a core): ";
    for (steal, said) in [(None, ""), (Some(0.2), ""), (Some(3.0), "; hypervisor steal 3% of those CPUs during the check")] {
        let t = Text::busiest_processes(lead, true, &who, steal);
        let local = format!("{lead}{}", strobe::validity::named_work(&plain, steal));
        c.text(&format!("busiest processes, steal {steal:?}"), &t, &local, &format!("2 other processes on the machine, the busiest 421% of a core{said}"), &names);
    }
    let none = Text::busiest_processes(lead, true, &[], Some(0.2));
    c.text("busiest processes: none named", &none, &format!("{lead}{}", strobe::validity::named_work(&[], Some(0.2))), &format!("{lead}none named (system work; 0% hypervisor steal)"), &[]);
    let one = msg!("the speed check ran slow; {}", Text::busiest_processes(lead, true, &who[..1], None));
    c.text("busiest processes: one", &one, &format!("the speed check ran slow; {lead}tracker (beta).exe 421%"), "the speed check ran slow; 1 other process on the machine, 421% of a core", &names[..1]);
    let elsewhere = msg!("pair 1 head: up to 94% of a core ({})", Text::busiest_processes("busiest elsewhere: ", false, &who, None));
    c.text(
        "busiest elsewhere, in parentheses",
        &elsewhere,
        "pair 1 head: up to 94% of a core (busiest elsewhere: tracker (beta).exe 421%, renderfarm.exe 273%)",
        "pair 1 head: up to 94% of a core (2 other processes, the busiest 421% of a core)",
        &names,
    );

    // A CPU reservation's line (bench's ReserveTotal::text).
    let mut moved = BTreeMap::new();
    for (n, k) in [("indexer", 3u32), ("updater", 1), ("chat-helper", 2), ("sync-agent", 1), ("backup", 1)] {
        moved.insert(private(n, fp), k);
    }
    let mut failed = BTreeMap::new();
    for (n, why) in [("guard.exe", "elevated"), ("vault.exe", "elevated"), ("sys-mon", "access denied")] {
        failed.insert(private(n, fp), why.to_string());
    }
    let t = msg!("other processes of this user kept off CPUs 16-31 during {} timed blocks: {:.0} moved per block{}; {}", 2, 3.0, Text::reserve_moved(&moved), Text::reserve_failed(&failed));
    c.text(
        "a CPU reservation",
        &t,
        "other processes of this user kept off CPUs 16-31 during 2 timed blocks: 3 moved per block (indexer ×3, chat-helper ×2, backup, sync-agent, …); 3 could not be moved: sys-mon (access denied); guard.exe, vault.exe (elevated)",
        "other processes of this user kept off CPUs 16-31 during 2 timed blocks: 3 moved per block; 3 could not be moved (access denied; elevated)",
        &["indexer", "chat-helper", "backup", "sync-agent", "guard", "vault", "sys-mon"],
    );

    let short = private("00008110-001A2B3C4D5E6F70", PrivKind::DeviceId).tail_chars(8);
    if short.kind() != PrivKind::DeviceId { c.failed += 1; println!("FAIL tail_chars changed the kind"); }

    // Refusal and Error: suffixed, append_detail, refusal lines' marks.
    let serial = private("R5CT20ABCDE", PrivKind::DeviceId);
    let r = Refusal::new(ValidatorId::Thermal, msg!("{} is at 41 °C", Text::device(&serial, false)));
    c.text("a refusal suffixed", &r.clone().suffixed(" (the block was filmed)").summary_text(), "R5CT20ABCDE is at 41 °C (the block was filmed)", "the phone is at 41 °C (the block was filmed)", &["R5CT20ABCDE"]);
    let r3 = r.clone().detail(msg!("read from {}", Text::device(&serial, false))).append_detail("cooling for 60 s");
    c.text("a refusal's detail appended to", &r3.message_text(), "Thermal: R5CT20ABCDE is at 41 °C; read from R5CT20ABCDE; cooling for 60 s", "Thermal: the phone is at 41 °C; read from the phone; cooling for 60 s", &["R5CT20ABCDE"]);
    let e = Error::from(r.clone()).append_detail("retried once");
    c.text("an error's refusal noted", &e.message_text(), "Thermal: R5CT20ABCDE is at 41 °C; retried once", "Thermal: the phone is at 41 °C; retried once", &["R5CT20ABCDE"]);
    if e.refusal.as_ref().and_then(|r| r.detail.clone()).as_deref() != Some("retried once") || e.hint.is_some() { c.failed += 1; println!("FAIL append_detail did not keep the refusal"); }
    let e2 = Error::msg(msg!("{} went away", Text::device(&serial, false))).append_detail("reconnect it").suffixed(" (twice)");
    c.text("an error noted and suffixed", &e2.message_text(), "R5CT20ABCDE went away; reconnect it (twice)", "the phone went away; reconnect it (twice)", &["R5CT20ABCDE"]);
    let mut lines = BTreeMap::new();
    lines.insert("frames.presented.p95".to_string(), r.clone());
    lines.insert("cpu.work".to_string(), Refusal::new(ValidatorId::NoMetric, "nothing measured it"));
    let spans = strobe::model::refusal_lines::spans(&lines);
    let line = Text::from_parts(r.message(), spans.get("frames.presented.p95").cloned().unwrap_or_default());
    c.text("a refused line's marks", &line, "Thermal: R5CT20ABCDE is at 41 °C", "Thermal: the phone is at 41 °C", &["R5CT20ABCDE"]);
    if spans.len() != 1 { c.failed += 1; println!("FAIL refused lines without marks got marks: {:?}", spans.keys().collect::<Vec<_>>()); }

    // Paths against homes and roots this machine does not have.
    use strobe::text::public_path_in as at;
    for (path, repo, state, home, public) in [
        ("/secret-mount", None, "/x/.local/share/strobe", "/", "…/secret-mount"),
        ("/root", None, "/root/.local/share/strobe", "/root", "~"),
        ("/private/var/folders/ab/T/repo/perf/a.ts", Some("/var/folders/ab/T/repo"), "/Users/ann/.local/share/strobe", "/Users/ann", "perf/a.ts"),
        ("/var/folders/ab/T/repo/perf/a.ts", Some("/private/var/folders/ab/T/repo"), "/Users/ann/.local/share/strobe", "/Users/ann", "perf/a.ts"),
        ("/home/ann/work/repo-northwind/a.ts", Some("/home/ann/work/repo"), "/home/ann/.local/share/strobe", "/home/ann", "~/…/a.ts"),
        (r"d:\\WORK\\Repo\\src\\a.ts", Some(r"D:\\work\\repo"), r"C:\\Users\\ann\\AppData\\Local\\strobe", r"C:\\Users\\ann", "src/a.ts"),
        (r"C:\\", None, r"C:\\Users\\ann\\AppData\\Local\\strobe", r"C:\\Users\\ann", "…"),
        ("~/clients/northwind/x.txt", None, "/home/ann/.local/share/strobe", "/home/ann", "~/…/x.txt"),
        ("/a/../../northwind/x", None, "/home/ann/.local/share/strobe", "/home/ann", "…/x"),
        ("/home/ann/.local/share/strobe/../../../northwind/x", None, "/home/ann/.local/share/strobe", "/home/ann", "~/…/x"),
    ] {
        let got = at(path, repo, state, home);
        let ok = got == public;
        c.failed += !ok as usize;
        println!("{} path {path:?} (home {home:?}): {got:?}{}", if ok { "ok  " } else { "FAIL" }, if ok { String::new() } else { format!(", expected {public:?}") });
    }
}

/// STROBE_DEV_CANARY=1 (\`on\`): every marked part's local text is tagged, no public text is, and the raw value is
/// not. Set to \`0\` or empty (\`!on\`): nothing is tagged.
fn canary(c: &mut Check, on: bool) {
    let serial = private("R5CT20ABCDE", PrivKind::DeviceId);
    let who = [(private("tracker.exe", PrivKind::ForeignProc), 42.0)];
    let mut failed = std::collections::BTreeMap::new();
    failed.insert(private("vault.exe", PrivKind::ForeignProc), "elevated".to_string());
    let deep = host::home().join("clients").join("northwind").join("x.txt");
    let r = Refusal::new(ValidatorId::Thermal, msg!("{} is hot", Text::device(&serial, false)));
    let texts = [
        ("redacted", serial.redacted("the phone")),
        ("quoted", private("Plan", PrivKind::WindowTitle).quoted_or("a window")),
        ("busiest", Text::busiest_processes("busiest: ", true, &who, None)),
        ("apps", Text::covering_apps(&[private("Mail Pro", PrivKind::ForeignProc)])),
        ("reserve", Text::reserve_failed(&failed)),
        ("path", Text::path(&deep, None)),
        ("url", Text::url("https://ann:pw@example.test/")),
        ("output", Text::external_output(&format!("cannot open {}", deep.display()), None)),
        ("short id", Text::device(&private("00008110-001A2B3C4D5E6F70", PrivKind::DeviceId).tail_chars(8), false)),
        ("refusal", r.message_text()),
        ("error", Error::from(r.clone()).message_text()),
    ];
    for (what, t) in &texts {
        let ok = t.local().contains("CANARY_") == on && !t.public().contains("CANARY_");
        c.failed += !ok as usize;
        println!("{} canary {}{what}: {:?} -> {:?}", if ok { "ok  " } else { "FAIL" }, if on { "" } else { "off: " }, t.local(), t.public());
    }
    if !on {
        return;
    }
    let short = &texts[8].1;
    if !short.local().ends_with("_4D5E6F70") { c.failed += 1; println!("FAIL the canary changed a private value itself: {:?}", short.local()); }
}

/// The review's inputs (cleanup/priv1 fdd8925): each must hide what it names.
fn review(c: &mut Check) {
    let serial = private("R5CT20ABCDE", PrivKind::DeviceId);
    // 1. Marks kept beside a field: an edit may hide too much, never too little.
    let mut r = Refusal::window_hidden(Hidden::Covered, msg!("the window is not visible; in front of it: {}", Text::covering_apps(&[private("win", PrivKind::ForeignProc)])));
    r.summary = r.summary.replace("not visible", "covered");
    c.hides("marks: a private word inside an ordinary word", &r.summary_text(), Some("[redacted]"), &["it: win", "appdow"]);
    let r = Refusal::new(ValidatorId::Thermal, private("head", PrivKind::DeviceId).redacted("the phone"));
    let r2 = Refusal { summary: format!("on head: {}", r.summary), ..r.clone() };
    c.hides("marks: the private word twice after a prefix", &r2.summary_text(), Some("[redacted]"), &["head"]);
    let r3 = Refusal { summary: format!("base ({}) and head ({})", r.summary, r.summary), ..r.clone() };
    c.hides("marks: the field copied in twice", &r3.summary_text(), Some("[redacted]"), &["head"]);
    let two = msg!("{} then {}", private("SECRET_A", PrivKind::HostName).redacted("a host"), private("SECRET_B", PrivKind::HostName).redacted("a host"));
    let mut r = Refusal::new(ValidatorId::Covered, two.clone());
    r.summary = "SECRET_B then SECRET_A".into();
    c.hides("marks: two marks reordered", &r.summary_text(), Some("[redacted]"), &["SECRET_A", "SECRET_B"]);
    r.summary = "SECRET_B then SECRET_A and SECRET_B".into();
    c.hides("marks: reordered, one twice", &r.summary_text(), Some("[redacted]"), &["SECRET_A", "SECRET_B"]);
    let run = Path::new(if cfg!(windows) { r"E:\\clients\\run" } else { "/opt/clients/run" });
    let mut r = Refusal::new(ValidatorId::Covered, "x").fix(msg!("delete {}", Text::path(run, None)));
    r.fix = Some("let the run finish, then delete clients/run and clients/run".into());
    c.hides("marks: a field overwritten wholesale", &r.fix_text().unwrap_or_default(), Some("[redacted]"), &["clients"]);
    let t = msg!("another window covers (10, 20): {} of process 4120", private("Northwind Secret Plan", PrivKind::WindowTitle).quoted_or("a window"));
    let mut r = Refusal::new(ValidatorId::Covered, t);
    r.summary = r.summary[..50].to_string();
    c.hides("marks: a field sliced through its mark", &r.summary_text(), Some("[redacted]"), &["Northwind"]);
    let mut r = Refusal::new(ValidatorId::Thermal, msg!("{} is at 41 °C", serial.redacted("the phone")));
    r.summary = r.summary.to_lowercase();
    c.hides("marks: a field lowercased", &r.summary_text(), Some("[redacted]"), &["r5ct20abcde"]);
    let mut r = Refusal::new(ValidatorId::Covered, msg!("covered by {}", private("Plan\\n  B", PrivKind::WindowTitle).redacted("another window")));
    r.summary = r.summary.split_whitespace().collect::<Vec<_>>().join(" ");
    c.hides("marks: whitespace collapsed", &r.summary_text(), Some("[redacted]"), &["Plan B", "Plan"]);
    let mut r = Refusal::new(ValidatorId::Covered, msg!("covered by {}", private("", PrivKind::WindowTitle).redacted("another window")));
    r.summary = r.summary.replace("covered", "hidden");
    c.hides("marks: an empty private value", &r.summary_text(), Some("[redacted]"), &["windowhidden"]);
    let r = Refusal::new(ValidatorId::Thermal, msg!("{} is hot", private("aa", PrivKind::DeviceId).redacted("the phone")));
    let r2 = Refusal { summary: format!("a{}", r.summary), ..r.clone() };
    c.hides("marks: an occurrence across the edit's edge", &r2.summary_text(), Some("[redacted]"), &["aa"]);
    let r = Refusal::new(ValidatorId::Thermal, msg!("{} is at 41 °C", serial.redacted("the phone")));
    let r2 = Refusal { summary: format!("for R5CT20ABCDE: {}", r.summary), ..r.clone() };
    c.hides("marks: the private word in the added prefix", &r2.summary_text(), Some("[redacted]"), &["R5CT20ABCDE"]);
    let r2 = Refusal { summary: format!("phone 2: {} (retried)", r.summary), ..r.clone() };
    c.hides("marks: a prefix and a suffix around it whole", &r2.summary_text(), Some("phone 2: the phone is at 41 °C (retried)"), &["R5CT20ABCDE"]);
    let r = Refusal::new(ValidatorId::Covered, msg!("covered by {}", private("Ära-Plan", PrivKind::WindowTitle).quoted_or("a window")));
    let r2 = Refusal { summary: format!("{} / {}", r.summary, r.summary), ..r.clone() };
    c.hides("marks: a multi-byte private word, the field doubled", &r2.summary_text(), Some("[redacted]"), &["Ära"]);
    let mut r2 = r.clone();
    r2.summary.clear();
    c.hides("marks: a field emptied", &r2.summary_text(), Some(""), &["Ära"]);
    let mut r2 = r.clone();
    r2.summary = r2.summary.to_uppercase();
    c.hides("marks: a multi-byte private word uppercased", &r2.summary_text(), Some("[redacted]"), &["ÄRA"]);
    let e = Error::from(r.clone());
    let e2 = Error { message: e.message.replace("covered", "hidden"), ..e.clone() };
    c.hides("marks: an error's message edited through ..e", &e2.message_text(), Some("[redacted]"), &["Ära"]);
    let broken: Refusal = serde_json::from_str(r#"{"validator":"Covered","summary":"aé secret","spans":{"summary":[{"at":[0,2],"kind":"window_title","public":"x"}]}}"#).expect("broken");
    c.hides("marks: read back off a character boundary", &broken.summary_text(), Some("[redacted]"), &["secret"]);

    // 2. Paths.
    let home = host::home();
    let repo = home.join("work").join("repo");
    let up = repo.join("..").join("northwind-secret").join("plan.txt");
    c.hides("path: .. out of the repository", &Text::path(&up, Some(repo.as_path())), Some("~/…/plan.txt"), &["northwind"]);
    let st = host::paths::state_path().join("..").join("..").join("..").join("northwind-secret").join("plan.txt");
    c.hides("path: .. out of the state folder", &Text::path(&st, None), None, &["northwind", "<state>"]);
    let empty = Text::path(Path::new(""), None);
    c.hides("path: empty", &empty, Some(""), &[]);
    if empty.is_marked() { c.failed += 1; println!("FAIL path: an empty path is marked"); }

    // 3. Private equality ignores the kind (a value read back from a file has none).
    let a = private("tracker.exe", PrivKind::ForeignProc);
    let b: Private<String> = serde_json::from_str(&serde_json::to_string(&a).expect("json")).expect("read back");
    let mut moved = std::collections::BTreeMap::new();
    *moved.entry(a.clone()).or_insert(0) += 1;
    *moved.entry(b.clone()).or_insert(0) += 1;
    if a != b || moved.len() != 1 { c.failed += 1; println!("FAIL private: a value read back is another key ({} keys)", moved.len()); } else { println!("ok   private: a value read back is the same key"); }

    // 4. Debug of a Text shows its public form.
    let t = msg!("covered by {}", private("Secret Plan", PrivKind::WindowTitle).quoted_or("a window"));
    c.hides("debug: {:?} of a Text", &Text::from(format!("{t:?}")), None, &["Secret Plan"]);

    // 5. URLs.
    for (url, private) in [
        ("file:///Users/ann/northwind/index.html", &["northwind", "ann"][..]),
        ("data:text/html,<h1>Northwind</h1>", &["Northwind"][..]),
        ("https://example.test/app?accessToken=eyJ123&clientSecret=abc9&v=1", &["eyJ123", "abc9"][..]),
        ("https://example.test/#/login#token=s3cr3t", &["s3cr3t"][..]),
        ("https://example.test/app;jsessionid=s3cr3t?v=1;token=t0k3n", &["s3cr3t", "t0k3n"][..]),
        ("https://ann:hunt#er2@example.test/app", &["hunt", "er2", "ann:"][..]),
        ("https://ann:hu?nt@example.test/app", &["hu?nt", "ann:"][..]),
        ("https://example.test/a?TOKEN=up9&X-Amz-Signature=sg7&x-api-key=k3y", &["up9", "sg7", "k3y"][..]),
        ("ws://ann:pw7@localhost:9222/devtools", &["pw7"][..]),
    ] {
        c.hides(&format!("url: {url}"), &Text::url(url), None, private);
    }
    c.hides("url: a plain parameter stays", &Text::url("https://example.test/app?v=1&page=2"), Some("https://example.test/app?v=1&page=2"), &[]);

    // 6. Tool output.
    let out = Text::external_output(r"error in D:\\work\\repo-northwind\\a.ts", Some(Path::new(r"D:\\work\\repo")));
    c.hides("output: a sibling folder sharing the repository's name", &out, None, &["northwind"]);
    let out = Text::external_output("\\x1b[36m/Users/ann/northwind/a.ts\\x1b[0m:3:1", None);
    c.hides("output: a path after an ANSI escape", &out, None, &["northwind", "ann"]);
    let out = Text::external_output("D:/work/repo/src/a.ts:3:1: boom", Some(Path::new(r"D:\\work\\repo")));
    c.hides("output: forward slashes under a Windows root", &out, Some("src/a.ts:3:1: boom"), &["work"]);
    for root in ["/opt/northwind-corp/bin/tool", "/Volumes/Northwind/src/a.ts", "/srv/northwind/app/a.ts", "/mnt/northwind/x/a.ts"] {
        c.hides(&format!("output: {root}"), &Text::external_output(&format!("cannot open {root}: denied"), None), None, &["northwind"]);
    }
    c.hides("output: a URL with credentials", &Text::external_output("fetch https://ann:pw9@example.test/x failed", None), None, &["pw9"]);
    c.hides("output: a share", &Text::external_output(r"copied \\\\fileserver\\northwind\\drop\\a.zip", None), None, &["northwind", "fileserver"]);
    c.hides("output: a folder name with a space", &Text::external_output("built /opt/Northwind Corp/v1/app in 3 s", None), Some("built … …/app in 3 s"), &["Northwind"]);
    c.hides("output: numbers and units stay", &Text::external_output("120 /s, 3/4 done, 10 ms/run", None), Some("120 /s, 3/4 done, 10 ms/run"), &[]);

    // 11. A Text argument is written whole; clip(0) is empty.
    let rel = Text::path(Path::new("perf/app.bench.ts"), None);
    c.hides("msg!: precision on a Text", &msg!("{:.5}", rel), Some("perf/app.bench.ts"), &[]);
    c.hides("clip(0)", &Text::from("abc").clip(0), Some(""), &[]);
}

fn main() {
    let mut c = Check { failed: 0, show: std::env::args().any(|a| a == "--show") };
    if let Some(mode) = std::env::args().find(|a| a.starts_with("--canary")) {
        canary(&mut c, mode == "--canary");
        if c.failed > 0 { println!("{} canary example(s) failed", c.failed); std::process::exit(1); }
        return;
    }
    let busy = [(private("tracker (beta).exe", PrivKind::ForeignProc), 421.4), (private("renderfarm.exe", PrivKind::ForeignProc), 273.0), (private("sync-agent", PrivKind::ForeignProc), 14.6)];
    let names = ["tracker (beta).exe", "renderfarm.exe", "sync-agent"];

    // Other processes and their shares, as validity::named_work lists them, in a Background warning.
    let shares = Text::process_shares(&busy);
    c.text("process shares", &shares, "tracker (beta).exe 421%, renderfarm.exe 273%, sync-agent 15%", "3 other processes, the busiest 421% of a core", &names);
    c.text("one process", &Text::process_shares(&busy[1..2]), "renderfarm.exe 273%", "1 other process, 273% of a core", &names[1..2]);
    let n = 2;
    let warning = msg!("other work ran on the measured CPUs during {} block(s): pair 1 head: up to 94% of a core (busiest elsewhere: {})", n, shares);
    c.text(
        "a warning holding them",
        &warning,
        "other work ran on the measured CPUs during 2 block(s): pair 1 head: up to 94% of a core (busiest elsewhere: tracker (beta).exe 421%, renderfarm.exe 273%, sync-agent 15%)",
        "other work ran on the measured CPUs during 2 block(s): pair 1 head: up to 94% of a core (busiest elsewhere: 3 other processes, the busiest 421% of a core)",
        &names,
    );
    // Multi-byte text before the marks, and a padded plain argument: the marks stay on their bytes.
    let wide = msg!("Größe · naïve ✓ «{:>6}» {}", "ab", warning);
    c.text(
        "after multi-byte text and padding",
        &wide,
        &format!("Größe · naïve ✓ «    ab» {}", warning.local()),
        &format!("Größe · naïve ✓ «    ab» {}", warning.public()),
        &names,
    );

    // A covering window: its title (Debug-quoted as today), the process id and the coordinates kept.
    let title = private("Plan — Q3 \\"draft\\"\\nnotes", PrivKind::WindowTitle);
    let (x, y, pid) = (10, 20, 4120);
    let cover = msg!("another window covers the point ({}, {}): {} of process {}", x, y, title.quoted_or("a window"), pid);
    c.text(
        "a covering window's title",
        &cover,
        &format!("another window covers the point (10, 20): {:?} of process 4120", "Plan — Q3 \\"draft\\"\\nnotes"),
        "another window covers the point (10, 20): a window of process 4120",
        &["Plan", "draft", "notes"],
    );
    let over = msg!("{} (pid {}) covers the region", Text::window(&title), pid);
    c.text("a window by its title", &over, &format!("{:?} (pid 4120) covers the region", "Plan — Q3 \\"draft\\"\\nnotes"), "another window (pid 4120) covers the region", &["Plan", "draft"]);

    // Apps in front of a window (macOS).
    let apps = [private("Mail Pro", PrivKind::ForeignProc), private("Notes+", PrivKind::ForeignProc)];
    let front = msg!("the window is not visible; in front of it: {}", Text::covering_apps(&apps));
    c.text("covering apps", &front, "the window is not visible; in front of it: Mail Pro, Notes+", "the window is not visible; in front of it: 2 other apps", &["Mail Pro", "Notes+"]);

    // A phone's serial and what strobe runs under.
    let serial = private("R5CT20ABCDE", PrivKind::DeviceId);
    c.text("a phone's serial", &msg!("{} is at 41 °C", serial.redacted("the phone")), "R5CT20ABCDE is at 41 °C", "the phone is at 41 °C", &["R5CT20ABCDE"]);
    let ancestor = private("/Applications/Term Deluxe.app", PrivKind::HostAncestor);
    c.text(
        "what strobe runs under",
        &msg!("timer coalescing is on for {}", ancestor.redacted("another service")),
        "timer coalescing is on for /Applications/Term Deluxe.app",
        "timer coalescing is on for another service",
        &["Term Deluxe"],
    );

    // Paths: in the repository, the state folder, the home folder, elsewhere, relative.
    let home = host::home();
    let repo = home.join("work").join("northwind-client").join("repo");
    let path = |p: &Path, public: &str, private: &[&str], what: &str, c: &mut Check| {
        let t = msg!("cannot read {}", Text::path(p, Some(repo.as_path())));
        let local = format!("cannot read {}", p.display());
        c.text(what, &t, &local, &format!("cannot read {public}"), private);
    };
    path(&repo.join("perf").join("app.bench.ts"), "perf/app.bench.ts", &["northwind"], "a path in the repository", &mut c);
    path(&repo, ".", &["northwind"], "the repository itself", &mut c);
    path(&host::paths::state_path().join("experiments").join("exp_1a2b3c").join("result.json"), "<state>/experiments/exp_1a2b3c/result.json", &[], "a path in the state folder", &mut c);
    path(&home.join("clients").join("northwind").join("secret plan.txt"), "~/…/secret plan.txt", &["northwind", "clients"], "a path under the home folder", &mut c);
    path(&home.join("notes.txt"), "~/notes.txt", &[], "a file in the home folder", &mut c);
    path(&home, "~", &[], "the home folder", &mut c);
    let elsewhere: PathBuf = if cfg!(windows) { PathBuf::from(r"E:\\Clients\\Northwind\\v1") } else { PathBuf::from("/opt/Northwind-corp/v1") };
    path(&elsewhere, "…/v1", &["Northwind"], "a path elsewhere", &mut c);
    let verbatim = PathBuf::from(format!(r"\\\\?\\{}", repo.join("perf").display()));
    if cfg!(windows) { path(&verbatim, "perf", &["northwind"], "a verbatim path in the repository", &mut c); }
    c.text("a relative path", &Text::path(Path::new("perf/app.bench.ts"), Some(repo.as_path())), "perf/app.bench.ts", "perf/app.bench.ts", &[]);
    c.text("a relative path out of its folder", &Text::path(Path::new("../northwind/v1"), Some(repo.as_path())), "../northwind/v1", "…/v1", &["northwind"]);

    // A URL with credentials and a token; one without keeps itself.
    let url = "https://ann:hunter2@example.test/app?v=1&token=abc123#/route?session=s3cr3t";
    c.text("a URL with secrets", &msg!("open {}", Text::url(url)), &format!("open {url}"), "open https://<redacted>@example.test/app?v=1&token=<redacted>#/route?session=<redacted>", &["hunter2", "abc123", "s3cr3t"]);
    c.text("a URL without", &Text::url("https://example.test/app?v=1"), "https://example.test/app?v=1", "https://example.test/app?v=1", &[]);

    // A tool's output: paths in the repository from its root, the home folder as ~.
    let sep = std::path::MAIN_SEPARATOR;
    let out = format!("{}{sep}src{sep}a.ts:3:1: error: cannot open {}", repo.display(), home.join("scratch").join("northwind.json").display());
    let t = Text::external_output(&out, Some(repo.as_path()));
    c.text("a tool's output", &t, &out, "src/a.ts:3:1: error: cannot open ~/…/northwind.json", &[]);
    if t.public().contains(&home.display().to_string()) { c.failed += 1; println!("FAIL a tool's output shows the home folder"); }

    // Cut short: never inside a mark.
    let cut = cover.clip(50);
    c.text("cut inside a mark", &cut, "another window covers the point (10, 20): …", "another window covers the point (10, 20): …", &[]);
    let cut = cover.clip(30);
    c.text("cut before a mark", &cut, "another window covers the poi…", "another window covers the poi…", &[]);

    // An Error and a Refusal: their texts nest, their copies through ..r keep their marks, result.json keeps them.
    let r = Refusal::window_hidden(Hidden::Covered, cover.clone()).detail(msg!("busiest processes: {}", shares)).fix("keep the window uncovered");
    let summary_public = "another window covers the point (10, 20): a window of process 4120";
    let busiest_public = "busiest processes: 3 other processes, the busiest 421% of a core";
    c.text("a refusal's message", &r.message_text(), &format!("WindowHidden: {}; busiest processes: {}", cover.local(), shares.local()), &format!("WindowHidden: {summary_public}; {busiest_public}"), &names);
    if r.message() != format!("WindowHidden: {}; {}", r.summary, r.detail.clone().unwrap_or_default()) { c.failed += 1; println!("FAIL message() is not today's text"); }
    let e = Error::from(r.clone());
    c.text("its error", &e.message_text(), &r.message(), &format!("WindowHidden: {summary_public}; {busiest_public}"), &names);
    let e2 = e.clone().prefixed("launch 2 of 5: ");
    c.text("the error prefixed", &e2.message_text(), &format!("launch 2 of 5: {}", e.message), &format!("launch 2 of 5: WindowHidden: {summary_public}; {busiest_public}"), &names);
    let in_msg = msg!("block 3 stopped: {}", e);
    c.text("the error in a message", &in_msg, &format!("block 3 stopped: {}", e.message), &format!("block 3 stopped: WindowHidden: {summary_public}; {busiest_public}"), &names);
    let copied = Refusal { summary: format!("on head: {}", r.summary), ..r.clone() };
    c.text("a ..r copy with a prefix", &copied.summary_text(), &format!("on head: {}", cover.local()), &format!("on head: {summary_public}"), &["Plan"]);
    let mut appended = r.clone();
    appended.summary += " (the block was filmed)";
    c.text("a summary appended to", &appended.summary_text(), &format!("{} (the block was filmed)", cover.local()), &format!("{summary_public} (the block was filmed)"), &["Plan"]);
    let mut rewritten = r.clone();
    rewritten.summary = rewritten.summary.replace("another window", "a window");
    c.text("a summary rewritten around its mark: hidden whole", &rewritten.summary_text(), &cover.local().replace("another window", "a window"), "[redacted]", &["Plan"]);
    let mut lost = r.clone();
    lost.summary = lost.summary.replace("Plan", "Plan B");
    let t = lost.summary_text();
    if t.spans().len() != 1 || t.spans()[0].at != [0, t.local().len()] { c.failed += 1; println!("FAIL a summary changed past finding is not marked whole: {:?}", t.spans()); } else { println!("ok   a summary changed past finding is marked whole"); }

    let json = serde_json::to_string(&r).expect("json");
    let back: Refusal = serde_json::from_str(&json).expect("read back");
    c.text("result.json read back", &back.message_text(), &r.message(), &format!("WindowHidden: {summary_public}; {busiest_public}"), &names);
    if c.show { println!("  json: {json}"); }
    let plain = Refusal::new(ValidatorId::Background, "other work kept the CPUs busy").fix("let it finish");
    let plain_json = serde_json::to_string(&plain).expect("json");
    let expected = r#"{"validator":"Background","summary":"other work kept the CPUs busy","fix":"let it finish"}"#;
    if plain_json != expected { c.failed += 1; println!("FAIL an unmarked refusal writes {plain_json}, expected {expected}"); } else { println!("ok   an unmarked refusal writes what it always wrote"); }
    let older: Refusal = serde_json::from_str(r#"{"validator":"Covered","summary":"x","hidden":"covered"}"#).expect("older");
    if older.summary != "x" || !older.spans.is_empty() { c.failed += 1; println!("FAIL an older refusal reads back changed"); }
    let broken: Refusal = serde_json::from_str(r#"{"validator":"Covered","summary":"aé","spans":{"summary":[{"at":[0,2],"kind":"window_title","public":"x"}]}}"#).expect("broken");
    if broken.summary_text().spans().first().map(|s| s.at) != Some([0, 3]) { c.failed += 1; println!("FAIL a mark off a character boundary is not the whole field"); } else { println!("ok   a mark off a character boundary marks the whole field"); }

    review(&mut c);
    review2(&mut c);
    fuzz(&mut c);
    sentences(&mut c);
    api(&mut c);
    if c.failed > 0 { println!("{} example(s) failed", c.failed); std::process::exit(1); }
}
`

const prelude = `#![allow(dead_code, unused_imports)]\nuse strobe::model::{Error, Hidden, Refusal, ValidatorId};\nuse strobe::msg;\nuse strobe::text::{PrivKind, Private, Text};\n`
rmSync(dir, { recursive: true, force: true })
mkdirSync(join(dir, "src", "bin"), { recursive: true })
const problems: string[] = []
try {
writeFileSync(
  join(dir, "Cargo.toml"),
  `[package]\nname = "private-text"\nversion = "0.0.0"\nedition = "2024"\npublish = false\n\n[dependencies]\nstrobe = { path = ${JSON.stringify(root.replace(/\\/g, "/"))} }\nserde_json = "1"\n\n[workspace]\n`,
)
copyFileSync(join(root, "Cargo.lock"), join(dir, "Cargo.lock"))
for (const [name, [body]] of Object.entries(fails)) writeFileSync(join(dir, "src", "bin", `fail_${name}.rs`), `${prelude}${body}\nfn main() {}\n`)
writeFileSync(join(dir, "src", "bin", "control.rs"), `${prelude}${control}\nfn main() {}\n`)
writeFileSync(join(dir, "src", "bin", "show.rs"), `${prelude}${examples}`)

  // Offline: the crate's dependencies are strobe's own, already fetched (Cargo.lock copied); nothing is downloaded.
  const build = Bun.spawnSync(["cargo", "build", "--offline", "--bins", "--keep-going", "--message-format=json"], { cwd: dir, env: { ...process.env, CARGO_TARGET_DIR: target }, stderr: "pipe" })
  const errors = new Map<string, { code: string; message: string; rendered: string }[]>()
  const built = new Map<string, string>()
  for (const line of build.stdout.toString().split("\n")) {
    if (!line.startsWith("{")) continue
    const m = JSON.parse(line)
    if (m.reason === "compiler-artifact" && m.executable) built.set(m.target.name, m.executable)
    if (m.reason !== "compiler-message" || m.message.level !== "error") continue
    const list = errors.get(m.target.name) ?? []
    list.push({ code: m.message.code?.code ?? "", message: m.message.message, rendered: m.message.rendered ?? "" })
    errors.set(m.target.name, list)
  }
  if (!built.has("control") && !errors.has("control")) throw new Error(`the check crate did not build:\n${build.stderr.toString().slice(-4000)}`)
  for (const [name, [, code, words]] of Object.entries(fails)) {
    const got = errors.get(`fail_${name}`) ?? []
    const hit = got.find((e) => e.code === code && (e.message + e.rendered).includes(words))
    if (hit) console.log(`ok   compile-fail ${name}: ${code} ${hit.message.split("\n")[0]}`)
    else problems.push(`compile-fail ${name}: expected ${code} (${words}), got ${got.length ? got.map((e) => `${e.code} ${e.message}`).join("; ") : "a build that passed"}`)
    if (show && hit) console.log(hit.rendered)
  }
  for (const name of ["control", "show"]) {
    const got = errors.get(name)
    if (got) problems.push(`${name} did not compile:\n${got.map((e) => e.rendered).join("\n")}`)
    else console.log(`ok   ${name} compiles`)
  }
  const exe = built.get("show")
  if (exe) {
    const { STROBE_DEV_CANARY: _, ...plain } = process.env
    const run = Bun.spawnSync([exe, ...(show ? ["--show"] : [])], { stdout: "inherit", stderr: "inherit", env: plain })
    if (run.exitCode !== 0) problems.push(`examples failed (exit ${run.exitCode})`)
    const canary = Bun.spawnSync([exe, "--canary"], { stdout: "inherit", stderr: "inherit", env: { ...process.env, STROBE_DEV_CANARY: "1" } })
    if (canary.exitCode !== 0) problems.push(`the canary failed (exit ${canary.exitCode})`)
    for (const off of ["0", ""]) {
      const quiet = Bun.spawnSync([exe, "--canary-off"], { stdout: "inherit", stderr: "inherit", env: { ...process.env, STROBE_DEV_CANARY: off } })
      if (quiet.exitCode !== 0) problems.push(`STROBE_DEV_CANARY=${JSON.stringify(off)} tagged text (exit ${quiet.exitCode})`)
    }
  }
} finally {
  // The snippets that must not compile go; the compiled dependencies stay warm in the target folder.
  rmSync(dir, { recursive: true, force: true })
}
if (problems.length) {
  console.error(problems.join("\n"))
  process.exit(1)
}
console.log("private text: every compile-fail snippet refused, the control compiles, every example as expected")
