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
        "other processes of this user kept off CPUs 16-31 during 2 timed blocks: 3 moved per block (indexer x3, chat-helper x2, backup, sync-agent, …); 3 could not be moved: sys-mon (access denied); guard.exe, vault.exe (elevated)",
        "other processes of this user kept off CPUs 16-31 during 2 timed blocks: 3 moved per block; 3 could not be moved (access denied; elevated)",
        &["indexer", "chat-helper", "backup", "sync-agent", "guard", "vault", "sys-mon"],
    );

    // A phone: its id, a short id derived without exposing it, an emulator.
    let udid = private("00008110-001A2B3C4D5E6F70", PrivKind::DeviceId);
    let short = udid.tail_chars(8);
    c.text("a short device id", &msg!("{} is locked", Text::device(&short, false)), &format!("{} is locked", strobe::host::ios::short_udid("00008110-001A2B3C4D5E6F70")), "the phone is locked", &["4D5E6F70"]);
    if short.kind() != PrivKind::DeviceId { c.failed += 1; println!("FAIL tail_chars changed the kind"); }
    c.text("an emulator", &msg!("{} is not a phone", Text::device(&private("emulator-5554", PrivKind::DeviceId), true)), "emulator-5554 is not a phone", "the emulator is not a phone", &["5554"]);

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

/// STROBE_DEV_CANARY=1: every marked part's local text is tagged, no public text is, and the raw value is not.
fn canary(c: &mut Check) {
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
        let ok = t.local().contains("CANARY_") && !t.public().contains("CANARY_");
        c.failed += !ok as usize;
        println!("{} canary {what}: {:?} -> {:?}", if ok { "ok  " } else { "FAIL" }, t.local(), t.public());
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
    c.hides("output: a folder name with a space", &Text::external_output("built /opt/Northwind Corp/v1/app in 3 s", None), Some("built …/app in 3 s"), &["Northwind"]);
    c.hides("output: numbers and units stay", &Text::external_output("120 /s, 3/4 done, 10 ms/run", None), Some("120 /s, 3/4 done, 10 ms/run"), &[]);

    // 11. A Text argument is written whole; clip(0) is empty.
    let rel = Text::path(Path::new("perf/app.bench.ts"), None);
    c.hides("msg!: precision on a Text", &msg!("{:.5}", rel), Some("perf/app.bench.ts"), &[]);
    c.hides("clip(0)", &Text::from("abc").clip(0), Some(""), &[]);
}

fn main() {
    let mut c = Check { failed: 0, show: std::env::args().any(|a| a == "--show") };
    if std::env::args().any(|a| a == "--canary") {
        canary(&mut c);
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
