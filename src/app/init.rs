//! `strobe init [<dir>] [--target web|server|native|electron|tty|cli] [--force]`: a first bench for a project,
//! from what its own files say (SPEC §5): a strobe.json linked to its schema and a runnable
//! `perf/smoke.bench.ts` (and the helper it needs), then the commands that check them. It reads generic
//! markers only (package.json's scripts, `main` and `bin`, a Cargo.toml's binaries, an index.html) and
//! never replaces a file that is there without `--force`.

use super::Output;
use crate::cli::Args;
use crate::model::{Error, Result};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Web,
    Server,
    Native,
    Electron,
    Tty,
    Cli,
}

impl Kind {
    const ALL: [Kind; 6] = [Kind::Web, Kind::Server, Kind::Native, Kind::Electron, Kind::Tty, Kind::Cli];

    fn name(self) -> &'static str {
        match self {
            Kind::Web => "web",
            Kind::Server => "server",
            Kind::Native => "native",
            Kind::Electron => "electron",
            Kind::Tty => "tty",
            Kind::Cli => "cli",
        }
    }

    fn parse(s: &str) -> Result<Kind> {
        Kind::ALL.into_iter().find(|k| k.name() == s).ok_or_else(|| Error::usage(format!("--target is web, server, native, electron, tty or cli, not {s}")))
    }
}

/// What init found in the folder.
struct Project {
    dir: PathBuf,
    /// Where a folder variant of it builds and runs: its checkout's top, else the folder (SPEC §9.5).
    root: PathBuf,
    /// The folder from `root`, `/`-separated; empty when they are one.
    rel: String,
    package: Option<Value>,
    /// The Cargo.toml's binaries: its `[[bin]]` names, else the package's when it has `src/main.rs`.
    bins: Vec<String>,
    index_html: bool,
    /// The package manager its lockfile names (bun without one: strobe runs bench files with Bun).
    pm: &'static str,
}

impl Project {
    fn read(dir: &Path) -> Result<Project> {
        let package = match std::fs::read_to_string(dir.join("package.json")) {
            Ok(t) => Some(serde_json::from_str::<Value>(&t).map_err(|e| Error::usage(format!("{}: not JSON: {e}", dir.join("package.json").display())))?),
            Err(_) => None,
        };
        let bins = std::fs::read_to_string(dir.join("Cargo.toml")).map(|t| cargo_bins(&t, dir)).unwrap_or_default();
        let root = crate::host::git::toplevel(dir).and_then(|r| real(&r)).unwrap_or_else(|| dir.to_path_buf());
        let rel = dir.strip_prefix(&root).map(|r| r.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/")).unwrap_or_default();
        let pm = [("bun.lock", "bun"), ("bun.lockb", "bun"), ("pnpm-lock.yaml", "pnpm"), ("yarn.lock", "yarn"), ("package-lock.json", "npm")]
            .into_iter()
            .find(|(f, _)| dir.join(f).is_file() || root.join(f).is_file())
            .map_or("bun", |(_, pm)| pm);
        Ok(Project { dir: dir.to_path_buf(), root, rel, package, bins, index_html: dir.join("index.html").is_file(), pm })
    }

    fn script(&self, name: &str) -> Option<&str> {
        self.package.as_ref()?["scripts"][name].as_str()
    }

    fn depends_on(&self, name: &str) -> bool {
        self.package.as_ref().is_some_and(|p| ["dependencies", "devDependencies"].iter().any(|d| p[d].get(name).is_some()))
    }

    /// `<pm> run <script>`.
    fn run(&self, script: &str) -> String {
        format!("{} run {script}", self.pm)
    }

    /// A path of the folder as the variant's root sees it.
    fn rooted(&self, path: &str) -> String {
        if self.rel.is_empty() { path.to_string() } else { format!("{}/{path}", self.rel) }
    }

    /// `<runtime> <file>` (`bin`) with the file from the variant's root.
    fn rooted_line(&self, line: &str) -> String {
        match line.split_once(' ') {
            Some((runtime, file)) => format!("{runtime} {}", self.rooted(file)),
            None => self.rooted(line),
        }
    }

    /// A command line run from the variant's root (a target's `build`) that must run in this folder.
    fn in_dir(&self, line: &str) -> String {
        if self.rel.is_empty() { line.to_string() } else { format!("cd {} && {line}", self.rel) }
    }

    /// The package's command-line program: its `bin` (the first, when it names several) and the runtime
    /// its first line names (Bun otherwise), from this folder.
    fn bin(&self) -> Option<(String, String)> {
        let p = self.package.as_ref()?;
        let (name, file) = match &p["bin"] {
            Value::String(f) => (p["name"].as_str().unwrap_or("cli").rsplit('/').next().unwrap_or("cli").to_string(), f.clone()),
            Value::Object(m) => m.iter().find_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))?,
            _ => return None,
        };
        let file = file.trim_start_matches("./").to_string();
        let first = std::fs::read_to_string(self.dir.join(&file)).ok().and_then(|t| t.lines().next().map(str::to_string)).unwrap_or_default();
        let runtime = if first.starts_with("#!") && first.contains("node") { "node" } else { "bun" };
        Some((name, format!("{runtime} {file}")))
    }

    /// The server's entry: package.json `main` or `module`, else the file its `start` script runs (either
    /// may be the build's output, made before the first block), else a usual name, when the file is there.
    fn server_entry(&self) -> Option<String> {
        let p = self.package.as_ref();
        let mut declared: Vec<String> = ["main", "module"].iter().filter_map(|k| p?[k].as_str().map(str::to_string)).collect();
        if let Some(start) = self.script("start") {
            let words: Vec<&str> = start.split_whitespace().collect();
            if let Some(i) = words.iter().position(|w| matches!(*w, "bun" | "node" | "tsx" | "ts-node" | "deno")) {
                declared.extend(words[i + 1..].iter().find(|w| !w.starts_with('-') && *w != &"run").map(|w| w.to_string()));
            }
        }
        let code = |f: &String| [".ts", ".js", ".mjs", ".cjs", ".mts", ".tsx", ".jsx"].iter().any(|x| f.ends_with(x));
        let built = self.script("build").is_some();
        let usual = ["server.ts", "server.js", "src/server.ts", "src/server.js", "src/index.ts", "src/index.js", "index.ts", "index.js"].map(String::from);
        declared
            .into_iter()
            .map(|f| f.trim_start_matches("./").to_string())
            .find(|f| code(f) && (built || self.dir.join(f).is_file()))
            .or_else(|| usual.into_iter().find(|f| self.dir.join(f).is_file()))
    }

    /// Whether the folder holds a page: an index.html where a web app keeps it, or a preview script.
    fn web_page(&self) -> bool {
        self.index_html || ["public", "src", "app", "static"].iter().any(|d| self.dir.join(d).join("index.html").is_file()) || self.script("preview").is_some()
    }

    /// The kind to bench when `--target` names none, from the folder's markers.
    fn guess(&self) -> Result<Kind> {
        if self.depends_on("electron") {
            return Ok(Kind::Electron);
        }
        if self.web_page() {
            return Ok(Kind::Web);
        }
        if self.bin().is_some() {
            return Ok(Kind::Cli);
        }
        if self.package.is_some() && self.server_entry().is_some() {
            return Ok(Kind::Server);
        }
        if self.script("build").is_some() {
            return Ok(Kind::Web);
        }
        if !self.bins.is_empty() {
            return Ok(Kind::Cli);
        }
        Err(Error::usage(format!("{} holds no package.json, Cargo.toml or index.html that says what to bench", self.dir.display()))
            .hint("name it: strobe init --target web|server|native|electron|tty|cli"))
    }
}

/// A Cargo.toml's binaries, read line by line: each `[[bin]]`'s `name`, else `[package]`'s `name`
/// when `src/main.rs` is there.
fn cargo_bins(text: &str, dir: &Path) -> Vec<String> {
    let mut section = String::new();
    let (mut package, mut bins) = (None, Vec::new());
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            section = line.to_string();
            continue;
        }
        let Some(value) = line.strip_prefix("name").map(str::trim_start).and_then(|r| r.strip_prefix('=')) else { continue };
        let value = value.trim().trim_matches('"').trim_matches('\'').to_string();
        match section.as_str() {
            "[[bin]]" => bins.push(value),
            "[package]" => package = Some(value),
            _ => {}
        }
    }
    if bins.is_empty()
        && let Some(p) = package.filter(|_| dir.join("src").join("main.rs").is_file())
    {
        bins.push(p);
    }
    bins
}

/// The folder as the OS resolves it, without Windows' `\\?\` prefix.
fn real(p: &Path) -> Option<PathBuf> {
    std::fs::canonicalize(p).ok().map(|c| PathBuf::from(c.display().to_string().trim_start_matches(r"\\?\")))
}

/// JSON with the strings escaped, for a TypeScript literal.
fn lit(s: &str) -> String {
    Value::String(s.to_string()).to_string()
}

const HEAD: &str = "// A first bench strobe init wrote: edit its cases to measure what your users do.
//   strobe bench perf/smoke.bench.ts --head . --check              each case once: does it work?
//   strobe bench perf/smoke.bench.ts --base <branch> --head .      did this branch change it? (timed)
";

const WEB_BENCH: &str = r#"import { bench, web } from "@hona/strobe"

export default bench({
  // The project's production build, served as strobe.json's `serve` says, in a browser strobe launches.
  targets: { app: web({ url: "/" }) },
  cases: {
    // The first load in a fresh browser profile until something in the page shows: launch.ready.
    load: { primary: "launch.ready", launch: { until: "body *" } },
    // The page loaded again in the open browser: the main thread's work for one load.
    reload: {
      primary: "main.work",
      run: async ({ page }) => {
        await page.goto("/")
        await page.waitFor("body *")
      },
    },
  },
})
"#;

const SERVE_TS: &str = r#"// Serves the production build for strobe (strobe.json `serve`): bun perf/serve.ts <port> [<folder>].
// Without a folder it serves the first of dist, build, out and public that holds an index.html, else this
// one. A path without a file gets index.html, as a single-page app's host answers.
import { existsSync, statSync } from "node:fs"
import { join, resolve, sep } from "node:path"

const [port = "4173", folder] = process.argv.slice(2)
const root = resolve(folder ?? ["dist", "build", "out", "public"].find((d) => existsSync(join(d, "index.html"))) ?? ".")
const file = (path: string) => {
  const p = resolve(root, "." + decodeURIComponent(path))
  if (p !== root && !p.startsWith(root + sep)) return undefined
  if (existsSync(p) && statSync(p).isFile()) return p
  return existsSync(join(p, "index.html")) ? join(p, "index.html") : undefined
}

Bun.serve({
  hostname: "127.0.0.1",
  port: Number(port),
  fetch(req) {
    const path = new URL(req.url).pathname
    const found = file(path) ?? (/\.\w+$/.test(path) ? undefined : join(root, "index.html"))
    return found && existsSync(found) ? new Response(Bun.file(found)) : new Response("not found", { status: 404 })
  },
})
console.log(`serving ${root} on http://127.0.0.1:${port}/`)
"#;

const SERVER_BENCH: &str = r#"import { bench, script } from "@hona/strobe"

let url = ""

export default bench({
  // The harness runs the server in a process of its own, which strobe measures; the runs load it from here.
  targets: { server: script("./server.ts"@OPTS@) },
  async setup(ctx) {
    url = await ctx.call<string>("start")
    // A request every 100 ms from strobe's own threads: how long another client waited while the runs loaded
    // it. Any answer below 500 counts; narrow it to a route of your own, such as a health check.
    await ctx.httpProbe({ url, expect: { status: [200, 499] } })
  },
  cases: {
    // A steady load, not a flood: 1000 requests from four clients, each waiting 10 ms between its own. The
    // server's main thread for that work, and how long the probe's client waited meanwhile.
    load: {
      primary: "cpu.main",
      metrics: ["http.latency.p95", "http.timeouts"],
      run: async () => {
        const client = async () => {
          for (let i = 0; i < 250; i++) {
            await (await fetch(url)).arrayBuffer()
            await Bun.sleep(10)
          }
        }
        await Promise.all([client(), client(), client(), client()])
      },
    },
  },
})
"#;

const SERVER_TS: &str = r#"// The harness strobe loads in a fresh Bun process per block (a script() target). The bench calls its exports
// with ctx.call, so the server runs in the process strobe measures.

/** Starts the server on a free port, which it reads from PORT; the address once it answers. */
export async function start(): Promise<string> {
  const free = Bun.serve({ hostname: "127.0.0.1", port: 0, fetch: () => new Response() })
  const port = free.port
  free.stop(true)
  process.env.PORT = String(port)
@START@
  const url = `http://127.0.0.1:${port}/`
  for (let i = 0; i < 100; i++) {
    try {
      await fetch(url)
      return url
    } catch {
      await Bun.sleep(100)
    }
  }
  throw new Error(`nothing answered ${url} in 10 s: does the server listen on process.env.PORT?`)
}
"#;

const NATIVE_BENCH: &str = r#"import { bench, native } from "@hona/strobe"

export default bench({
  // The app strobe.json's `build` makes for each variant, launched in a fresh isolated home. An app that
  // opens no window (a tray app, a service): native(@APP@, { window: false }).
  targets: { app: native(@APP@) },
  cases: {
    // The app left open for a second: its CPU and thread wake-ups while nothing happens.
    idle: { primary: "cpu.work", metrics: ["cpu.wakes"], run: ({ page }) => page.wait(1000) },
  },
})
"#;

const ELECTRON_BENCH: &str = r#"import { bench, command } from "@hona/strobe"

export default bench({
  // The app from each variant's folder, started with the project's own Electron and attached through the
  // debugging port it opens; the build runs once per variant first.
  targets: { app: command("@RUN@", { build: @BUILD@ }) },
  cases: {
    // The app left open for a second: the page's main-thread work while nothing happens.
    idle: { primary: "main.work", metrics: ["cpu.work"], run: ({ page }) => page.wait(1000) },
  },
})
"#;

const TTY_BENCH: &str = r#"import { bench, tty } from "@hona/strobe"

@EXE@export default bench({
  // The program on strobe's own terminal, in a fresh isolated home per block: no display needed.
  targets: { term: tty(@RUN@@OPTS@) },
  cases: {
    // The program left open for a second: its CPU and wake-ups, and what it drew meanwhile.
    idle: { primary: "cpu.work", metrics: ["cpu.wakes", "tty.updates"], run: ({ page }) => page.wait(1000) },
  },
})
"#;

const CLI_BENCH: &str = r#"import { bench, cli } from "@hona/strobe"

@EXE@export default bench({
  // The command, run to its exit in each variant's folder (a nonzero exit fails the run): use the arguments
  // your users give it.
  targets: { cli: cli(@RUN@@OPTS@) },
  cases: {
    // One run of the command: spawn to exit, its CPU with everything it starts, and its peak memory.
    run: { primary: "run.duration", metrics: ["cpu.work", "memory.peak"], run: (ctx) => ctx.exec() },
  },
})
"#;

/// The files one kind writes: strobe.json's body (without `$schema`), the bench, and its helper.
struct Plan {
    config: Vec<(&'static str, String)>,
    bench: String,
    helper: Option<(&'static str, String)>,
    /// What init made of the folder, for the summary.
    what: String,
}

/// A Cargo binary's path from the variant's root (`./` first off Windows), its build into the folder's
/// `target` (from the root too, where a target's `build` runs), and the TypeScript constant the bench names
/// it by.
fn cargo_exe(p: &Project, bin: &str) -> (String, String) {
    let dir = p.rooted("target/release");
    let build = if p.rel.is_empty() { "cargo build --release --target-dir target".to_string() } else { format!("cargo build --release --manifest-path {0}/Cargo.toml --target-dir {0}/target", p.rel) };
    let exe = format!(
        "// The binary Cargo builds into {dir} (`build` below), once per variant.\nconst exe = process.platform === \"win32\" ? {} : {}\n\n",
        lit(&format!("{dir}/{bin}.exe")),
        lit(&format!("./{dir}/{bin}"))
    );
    (exe, build)
}

fn plan(kind: Kind, p: &Project) -> Result<Plan> {
    let head = HEAD.to_string();
    match kind {
        Kind::Web => {
            let build = p.script("build").map(|_| p.run("build"));
            if build.is_none() && !p.web_page() {
                return Err(Error::usage(format!("a web bench serves the project's build, and {} has no package.json build script and no index.html", p.dir.display()))
                    .hint("add a build script that writes the page (with an index.html), or point init at the folder that holds it"));
            }
            let mut config = Vec::new();
            if let Some(b) = &build {
                let mut step = json!({ "command": b });
                if !p.rel.is_empty() {
                    step["cwd"] = json!(p.rel);
                }
                config.push(("build", pretty(&step, 2)));
            }
            let mut serve = json!({ "command": "bun perf/serve.ts {port}" });
            if !p.rel.is_empty() {
                serve["cwd"] = json!(p.rel);
            }
            let mut serve_text = pretty(&serve, 2);
            // The app's own server instead, when it has one: a preview of the build (not a dev server).
            if let Some(s) = ["preview", "start"].into_iter().find(|s| p.script(s).is_some()) {
                serve_text = serve_text.replacen('{', &format!("{{\n    // or the project's own: \"command\": \"{} -- --port {{port}}\" (a server of the build, never a dev server)", p.run(s)), 1);
            }
            config.push(("serve", serve_text));
            let what = match &build {
                Some(b) => format!("a web app: {b}, served by perf/serve.ts"),
                None => "a web page (index.html), served by perf/serve.ts".to_string(),
            };
            Ok(Plan { config, bench: head + WEB_BENCH, helper: Some(("serve.ts", SERVE_TS.to_string())), what })
        }
        Kind::Server => {
            let entry = p.server_entry();
            let start = match &entry {
                Some(e) => format!("  // The project's server (package.json main or start); it must listen on process.env.PORT.\n  await import({})", lit(&format!("../{e}"))),
                None => "  // No server entry found (package.json main or start): a stand-in that answers. Import yours instead:\n  // await import(\"../src/server.ts\"), a server that listens on process.env.PORT.\n  Bun.serve({ hostname: \"127.0.0.1\", port, fetch: () => Response.json({ status: \"ok\" }) })".to_string(),
            };
            // A build (TypeScript to JavaScript, a bundle) runs once per variant before its first block.
            let build = p.script("build").map(|_| p.in_dir(&p.run("build")));
            let opts = build.as_ref().map(|b| format!(", {{ build: {} }}", lit(b))).unwrap_or_default();
            let what = match &entry {
                Some(e) => format!("a server: {e}{}, hosted by perf/server.ts", build.as_ref().map(|b| format!(" (after {b})")).unwrap_or_default()),
                None => "a server: a stand-in in perf/server.ts (no entry found)".to_string(),
            };
            Ok(Plan { config: Vec::new(), bench: head + &SERVER_BENCH.replace("@OPTS@", &opts), helper: Some(("server.ts", SERVER_TS.replace("@START@", &start))), what })
        }
        Kind::Native => {
            let bin = p.bins.first().ok_or_else(|| {
                Error::usage(format!("a native bench builds the app, and {} has no Cargo.toml binary", p.dir.display()))
                    .hint("for another build, write the app's profile in strobe.json apps (kind native, its exe) and its build (command, output)")
            })?;
            let per_os = |path: &str| json!({ "windows": format!("{path}.exe"), "macos": path, "linux": path });
            let mut build = json!({
                "command": "cargo build --release",
                "output": per_os(&format!("{{cache}}/cargo-target/release/{bin}")),
                "env": {
                    "CARGO_TARGET_DIR": "{cache}/cargo-target",
                    "CARGO_PROFILE_RELEASE_DEBUG": "line-tables-only",
                    "CARGO_PROFILE_RELEASE_STRIP": "false",
                    "CARGO_PROFILE_RELEASE_INCREMENTAL": "false"
                }
            });
            if !p.rel.is_empty() {
                build["cwd"] = json!(p.rel);
            }
            let app = json!({ bin.as_str(): { "kind": "native", "exe": per_os(&format!("target/release/{bin}")) } });
            let apps = pretty(&app, 2).replacen('{', "{\n    // the profile app:<id> and native(\"<id>\") use; \"window\": false for an app that opens none", 1);
            let what = format!("a native app: the Cargo binary {bin}{}", if p.bins.len() > 1 { format!(" (also {})", p.bins[1..].join(", ")) } else { String::new() });
            Ok(Plan { config: vec![("build", pretty(&build, 2)), ("apps", apps)], bench: head + &NATIVE_BENCH.replace("@APP@", &lit(bin)), helper: None, what })
        }
        Kind::Electron => {
            if !p.depends_on("electron") {
                return Err(Error::usage(format!("{} depends on no electron package", p.dir.join("package.json").display()))
                    .hint("an installed Electron app: strobe record exe:<path>, or an app profile in strobe.json apps"));
            }
            let install = format!("{} install", p.pm);
            let build = p.in_dir(&match p.script("build") {
                Some(_) => format!("{install} && {}", p.run("build")),
                None => install,
            });
            let run = format!("{} electron . --remote-debugging-port={{port}}", if p.pm == "bun" { "bunx" } else { "npx" });
            let cwd = if p.rel.is_empty() { String::new() } else { format!(", cwd: {}", lit(&p.rel)) };
            let what = format!("an Electron app: {run}");
            Ok(Plan { config: Vec::new(), bench: head + &ELECTRON_BENCH.replace("@RUN@", &run).replace("@BUILD@", &format!("{}{cwd}", lit(&build))), helper: None, what })
        }
        Kind::Tty | Kind::Cli => {
            // Both run in the variant's root, so their paths are from there.
            let cli = kind == Kind::Cli;
            let opts = |build: Option<String>| build.map(|b| format!(", {{ build: {} }}", lit(&b))).unwrap_or_default();
            let (exe, run, opts, what) = if let Some((name, line)) = p.bin() {
                let line = p.rooted_line(&line);
                let line = if cli { format!("{line} --help") } else { line };
                (String::new(), lit(&line), opts(None), format!("the package's program {name}"))
            } else if let Some(bin) = p.bins.first() {
                let (exe, build) = cargo_exe(p, bin);
                let run = if cli { "`${exe} --help`".to_string() } else { "exe".to_string() };
                (exe, run, opts(Some(build)), format!("the Cargo binary {bin}"))
            } else if !cli && p.script("start").is_some() {
                (String::new(), lit(&p.in_dir(&p.run("start"))), opts(None), "the package's start script".to_string())
            } else {
                return Err(Error::usage(format!("a {} bench runs the project's program, and {} names none (package.json bin, a Cargo.toml binary)", kind.name(), p.dir.display()))
                    .hint("write the command in the bench yourself: cli(\"<command> <args>\") or tty(\"<command>\")"));
            };
            let template = if kind == Kind::Cli { CLI_BENCH } else { TTY_BENCH };
            let what = format!("a {} program: {what}", if kind == Kind::Cli { "command-line" } else { "terminal" });
            Ok(Plan { config: Vec::new(), bench: head + &template.replace("@EXE@", &exe).replace("@RUN@", &run).replace("@OPTS@", &opts), helper: None, what })
        }
    }
}

/// `value` as indented JSON, its lines after the first indented by `indent` spaces.
fn pretty(value: &Value, indent: usize) -> String {
    serde_json::to_string_pretty(value).unwrap_or_default().replace('\n', &format!("\n{}", " ".repeat(indent)))
}

/// The `@hona/strobe` package this folder resolves (it, or an ancestor's `node_modules`).
fn installed(dir: &Path) -> Option<PathBuf> {
    dir.ancestors().map(|d| d.join("node_modules").join("@hona").join("strobe")).find(|p| p.is_dir())
}

pub fn run(args: &Args) -> Result<Output> {
    let dir = match args.pos.as_slice() {
        [] => std::env::current_dir()?,
        [d] => PathBuf::from(d),
        [_, extra, ..] => return Err(Error::usage(format!("init takes one folder at most, not also {extra}"))),
    };
    if !dir.is_dir() {
        return Err(Error::usage(format!("{} is no folder", dir.display())));
    }
    let given = args.get("target").map(|t| Kind::parse(t.trim())).transpose()?;
    let dir = real(&dir).ok_or_else(|| Error::usage(format!("{} cannot be read", dir.display())))?;
    let p = Project::read(&dir)?;
    let kind = match given {
        Some(k) => k,
        None => p.guess()?,
    };
    let plan = plan(kind, &p)?;
    let force = args.has("force");
    let package = installed(&dir);
    let schema = match &package {
        Some(pkg) => {
            let rel = pkg.strip_prefix(&dir).map(|r| r.to_string_lossy().replace('\\', "/")).ok();
            match rel {
                Some(r) => format!("./{r}/schema/strobe.v1.json"),
                None => {
                    let up = dir.ancestors().position(|a| pkg.starts_with(a)).unwrap_or(0);
                    format!("{}node_modules/@hona/strobe/schema/strobe.v1.json", "../".repeat(up))
                }
            }
        }
        None => "./node_modules/@hona/strobe/schema/strobe.v1.json".to_string(),
    };
    let mut config = format!("{{\n  \"$schema\": {}", lit(&schema));
    for (k, v) in &plan.config {
        config += &format!(",\n  \"{k}\": {v}");
    }
    config += "\n}\n";
    let mut files: Vec<(PathBuf, String)> = vec![(dir.join("strobe.json"), config), (dir.join("perf").join("smoke.bench.ts"), plan.bench)];
    if let Some((name, text)) = plan.helper {
        files.push((dir.join("perf").join(name), text));
    }
    let shown = |f: &Path| f.strip_prefix(&dir).map(|r| r.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| f.display().to_string());
    let (mut wrote, mut kept) = (Vec::new(), Vec::new());
    let mut config_differs = false;
    for (path, text) in &files {
        if path.exists() && !force {
            kept.push(shown(path));
            config_differs |= path.ends_with("strobe.json") && std::fs::read_to_string(path).ok().as_ref() != Some(text);
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, text)?;
        wrote.push(shown(path));
    }
    // A page with no display to show on (a locked session, Linux without one) runs headless.
    let headless = matches!(kind, Kind::Web) && (crate::target::chromium::no_display().is_some() || crate::host::session_locked());
    let strobe = if package.is_some() { "bunx strobe" } else { "strobe" };
    let cwd = std::env::current_dir().ok().and_then(|c| real(&c));
    let mut next = Vec::new();
    if cwd.as_deref() != Some(dir.as_path()) {
        let shown = dir.display().to_string();
        let shown = if shown.contains(' ') { format!("\"{shown}\"") } else { shown };
        next.push((format!("cd {shown}"), "the commands below run from the project".to_string()));
    }
    next.push((format!("{strobe} config"), "strobe.json as strobe reads it: builds, server, app profiles".to_string()));
    next.push((
        format!("{strobe} bench perf/smoke.bench.ts --head . --check{}", if headless { " --headless" } else { "" }),
        if headless { "each case once: does it work? (headless: no display here, or the session is locked)".to_string() } else { "each case once: does it work?".to_string() },
    ));
    // A timed comparison against the branch the checkout's remote names, else main or master; outside git an A/A.
    let base = crate::host::git::out(&p.root, &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"])
        .or_else(|| ["main", "master"].into_iter().find(|b| crate::host::git::ok(&p.root, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{b}")])).map(str::to_string));
    next.push(match &base {
        Some(b) => (format!("{strobe} bench perf/smoke.bench.ts --base {b} --head ."), "after a change: did it help? (timed, on a quiet machine)".to_string()),
        None => (format!("{strobe} bench perf/smoke.bench.ts --aa --head ."), "how far the numbers move by themselves here (timed; with git, --base <branch> compares)".to_string()),
    });
    let mut text = format!("project  {} · {}\n", dir.display(), plan.what);
    for f in &wrote {
        text += &format!("wrote    {f}\n");
    }
    for f in &kept {
        text += &format!("kept     {f} (there already; --force replaces it)\n");
    }
    // The bench leans on what init would have put in strobe.json: say so when the file there was kept.
    if config_differs && !plan.config.is_empty() {
        let keys: Vec<&str> = plan.config.iter().map(|(k, _)| *k).collect();
        text += &format!("note     the bench needs strobe.json's {}: check yours has them (strobe config), or --force\n", keys.join(" and "));
    }
    if package.is_none() {
        text += "note     no @hona/strobe in node_modules: bun add -d @hona/strobe pins strobe for the project and types the bench\n";
    }
    if kind == Kind::Native && cfg!(target_os = "linux") && std::env::var_os("DISPLAY").is_none_or(|v| v.is_empty()) {
        text += "note     no X11 display here: strobe runs the app on a Wayland compositor of its own, so it must speak Wayland (or run strobe under xvfb-run -a)\n";
    }
    let width = next.iter().map(|(c, _)| c.chars().count()).max().unwrap_or(0);
    for (i, (cmd, why)) in next.iter().enumerate() {
        text += &format!("{}{cmd:<width$}  {why}\n", if i == 0 { "next     " } else { "         " });
    }
    let json = json!({
        "dir": dir, "root": p.root, "target": kind.name(), "what": plan.what, "wrote": wrote, "kept": kept, "schema": schema,
        "headless": headless, "next": next.iter().map(|(c, _)| c).collect::<Vec<_>>(),
    });
    Ok(Output::new(text, json))
}
