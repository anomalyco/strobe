// Private names marked where a message is composed (src/text.rs; SPEC 1 rule 9), checked by the compiler and on real
// strings (dev, not shipped). It builds a small crate against strobe's library in <target>/private-text and:
//
//   compile-fail  each snippet below must fail to compile with its error: a `Private` formatted with `{}`, `{:?}` or
//                 `{p}`, turned into a string, read through `expose_local()` from outside, or given to `msg!`; a path's
//                 `display()` given to `msg!`; a `{:?}` argument in `msg!`; a `Text` formatted with `{}`;
//   compiles      the control: every kind of argument `msg!` takes, and `Error`/`Refusal` called as every caller
//                 calls them today (plain strings, `impl Into<String>`), compile;
//   examples      each constructor on real strings (other processes and their shares, a covering window's title,
//                 covering apps, paths in the repository, the state folder, the home folder and elsewhere, a URL
//                 with credentials and a token, a tool's output), nested in messages with multi-byte text before
//                 the marks, cut short, padded, through an `Error`, a `Refusal`, its `..r` copies and result.json:
//                 each local rendering is today's text, each public one the expected words, counts and process ids
//                 kept, no private part left.
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
}

const control = `
fn plain(s: impl Into<String>) -> Error { Error::msg(s) }
fn f(t: &Text, e: &Error, r: &Refusal, p: &Private<String>, n: u32, x: f64) -> Vec<Text> {
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
        strobe::msg!("inline {n} {pid} {x:.0}"),
        strobe::msg!("{}; {}; {}; {}", t, e, r, ValidatorId::Covered),
        strobe::msg!("{} of process {pid}", p.quoted_or("a window")),
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

fn private(s: &str, kind: PrivKind) -> Private<String> { Private::new(s.to_string(), kind) }

fn main() {
    let mut c = Check { failed: 0, show: std::env::args().any(|a| a == "--show") };
    let busy = [(private("tracker (beta).exe", PrivKind::ForeignProc), 421.4), (private("renderfarm.exe", PrivKind::ForeignProc), 273.0), (private("sync-agent", PrivKind::ForeignProc), 14.6)];
    let names = ["tracker (beta).exe", "renderfarm.exe", "sync-agent"];

    // Other processes and their shares, as validity::named_work lists them, in a Background warning.
    let shares = Text::process_shares(&busy);
    c.text("process shares", &shares, "tracker (beta).exe 421%, renderfarm.exe 273%, sync-agent 15%", "3 other processes, the busiest 421% of a core", &names);
    c.text("one process", &Text::process_shares(&busy[1..2]), "renderfarm.exe 273%", "1 other process, 273% of a core", &names[1..2]);
    let n = 2;
    let warning = msg!("other work ran on the measured CPUs during {n} block(s): pair 1 head: up to 94% of a core (busiest elsewhere: {})", shares);
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
    let cover = msg!("another window covers the point ({x}, {y}): {} of process {pid}", title.quoted_or("a window"));
    c.text(
        "a covering window's title",
        &cover,
        &format!("another window covers the point (10, 20): {:?} of process 4120", "Plan — Q3 \\"draft\\"\\nnotes"),
        "another window covers the point (10, 20): a window of process 4120",
        &["Plan", "draft", "notes"],
    );
    let over = msg!("{} (pid {pid}) covers the region", Text::window(&title));
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
    c.text("a tool's output", &t, &out, &format!("src{sep}a.ts:3:1: error: cannot open ~{sep}scratch{sep}northwind.json"), &[]);
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
    c.text("a summary rewritten around its mark", &rewritten.summary_text(), &cover.local().replace("another window", "a window"), "a window covers the point (10, 20): a window of process 4120", &["Plan"]);
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

    if c.failed > 0 { println!("{} example(s) failed", c.failed); std::process::exit(1); }
}
`

const prelude = `#![allow(dead_code, unused_imports)]\nuse strobe::model::{Error, Hidden, Refusal, ValidatorId};\nuse strobe::msg;\nuse strobe::text::{PrivKind, Private, Text};\n`
rmSync(join(dir, "src"), { recursive: true, force: true })
mkdirSync(join(dir, "src", "bin"), { recursive: true })
writeFileSync(
  join(dir, "Cargo.toml"),
  `[package]\nname = "private-text"\nversion = "0.0.0"\nedition = "2024"\npublish = false\n\n[dependencies]\nstrobe = { path = ${JSON.stringify(root.replace(/\\/g, "/"))} }\nserde_json = "1"\n\n[workspace]\n`,
)
copyFileSync(join(root, "Cargo.lock"), join(dir, "Cargo.lock"))
for (const [name, [body]] of Object.entries(fails)) writeFileSync(join(dir, "src", "bin", `fail_${name}.rs`), `${prelude}${body}\nfn main() {}\n`)
writeFileSync(join(dir, "src", "bin", "control.rs"), `${prelude}${control}\nfn main() {}\n`)
writeFileSync(join(dir, "src", "bin", "show.rs"), `${prelude}${examples}`)

const build = Bun.spawnSync(["cargo", "build", "--bins", "--keep-going", "--message-format=json"], { cwd: dir, env: { ...process.env, CARGO_TARGET_DIR: target }, stderr: "pipe" })
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
const problems: string[] = []
if (!built.has("control") && !errors.has("control")) {
  console.error(`the check crate did not build:\n${build.stderr.toString().slice(-4000)}`)
  process.exit(1)
}
for (const [name, [, code, words]] of Object.entries(fails)) {
  const got = errors.get(`fail_${name}`) ?? []
  const hit = got.find((e) => e.code === code && (e.message + e.rendered).includes(words))
  if (hit) console.log(`ok   compile-fail ${name}: ${code} ${hit.message}`)
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
  const run = Bun.spawnSync([exe, ...(show ? ["--show"] : [])], { stdout: "inherit", stderr: "inherit" })
  if (run.exitCode !== 0) problems.push(`examples failed (exit ${run.exitCode})`)
}
if (problems.length) {
  console.error(problems.join("\n"))
  process.exit(1)
}
console.log("private text: every compile-fail snippet refused, the control compiles, every example as expected")
