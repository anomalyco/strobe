// Rule 9 (SPEC 1): what belongs to the machine is marked where a message is composed (src/text.rs). The compiler
// keeps a `Private` value out of text unmarked only while these hold; paths, URLs and tools' output it does not
// see at all, so the lints below are part of what keeps them. Fails when:
//   (a) `Private` gains `Display` or `Debug` (derived or written, under any path), or `Text` gains `Display`:
//       `format!` would take either whole, its private parts unmarked;
//   (b) `IntoPiece` (what `msg!` takes) is implemented for `Private`, a path or a path's `Display`;
//   (c) `expose_local` is called or named (`Private::expose_local`) outside the files listed below, each with
//       a reason, or the list names a file that must never expose a private value (validity, model, stats,
//       text, driver, a11y, the public surfaces);
//   (d) a `msg!` call (any delimiter) formats a path itself (`.display()`, `.to_string_lossy()`, `.to_str()`,
//       `paths::plain`) where `Text::path` says it, takes a `Text`'s local string (`.local()`, `.into_local()`)
//       where it would take the `Text`, or has a format string (plain or raw) with a placeholder other than
//       `{}`, `{0}` and `{:spec}` without `?`, `*` or `$` (the compiler refuses those too);
//   (e) an OS call that reads another program's or the machine's names (`GetWindowTextW`, `GetClassNameW`,
//       `kCGWindowOwnerName`, `USERPROFILE`, `COMPUTERNAME`, `gethostname`) appears outside the files that
//       are its entry points;
//   (f) src/text.rs or src/text/ names anything of the crate but `host::home` and `host::paths` (it sits below
//       `model`; SPEC 4.3: nothing outside app/, cli and main names app).
// It reads files only (no build). `bun scripts/dev/private-text.ts` proves the compiler half; `--self-test` runs
// these checks on known bypasses instead of src/.
//
//   bun scripts/privacy-check.ts [--self-test]
import { readdirSync, readFileSync, statSync } from "node:fs"
import { join, relative } from "node:path"

const root = join(import.meta.dir, "..")

// Where `expose_local()` may hand out a private value, and why: what never becomes text (an OS call, a wire
// protocol, a comparison with another of the machine's names).
const exposeAllowed: Record<string, string> = {}

// Files that compose or publish text: none may ever expose a private value, whatever the list above says.
const neverExpose = [/^src\/validity\.rs$/, /^src\/model\.rs$/, /^src\/stats\.rs$/, /^src\/text(\.rs|\/)/, /^src\/driver(\.rs|\/)/, /^src\/target\/a11y\//, /^src\/app\/pr(\.rs|\/)/, /^src\/app\/card(\.rs|\/)/, /^src\/app\/report\.rs$/]

// OS calls that read names of this machine or of other programs, and the entry points allowed to make them.
const osNames: Record<string, string[]> = {
  GetWindowTextW: ["src/host/win.rs", "src/target/a11y/input.rs"],
  GetClassNameW: ["src/target/a11y/input.rs"],
  kCGWindowOwnerName: ["src/host/macos.rs"],
  kCGWindowOwnerPID: ["src/host/macos.rs", "src/probe/present/sck.rs", "src/target/a11y/ax.rs", "src/target/a11y/cg_input.rs"],
  responsibility_get_pid_responsible_for_pid: ["src/host/macos.rs"],
  AXUIElementCopyElementAtPosition: ["src/target/a11y/ax.rs"],
  WindowFromPhysicalPoint: ["src/target/a11y/input.rs"],
  GetNamedPipeServerProcessId: ["src/probe/helper.rs"],
  USERPROFILE: ["src/host/mod.rs", "src/target/isolate.rs"],
  COMPUTERNAME: ["src/host/mod.rs"],
  gethostname: ["src/host/mod.rs"],
}

// What src/text.rs may name of the crate.
const textMayName = [/^crate::host::home\b/, /^crate::host::paths::/, /^crate::text::/]

const walk = (d: string): string[] =>
  readdirSync(d).flatMap((n) => {
    const f = join(d, n)
    return statSync(f).isDirectory() ? walk(f) : f.endsWith(".rs") ? [f] : []
  })
const sources = walk(join(root, "src")).map((f) => ({ file: relative(root, f).replace(/\\/g, "/"), text: readFileSync(f, "utf8").replace(/\r\n/g, "\n") }))
const problems: string[] = []
const at = (text: string, i: number) => text.slice(0, i).split("\n").length

// The source twice, each the same length: `code` with comments blanked (strings and character literals kept),
// `bare` with their contents blanked too. A sentence about `expose_local()` is no use of it, a `//` inside a
// string is no comment, and a `)` inside a string or a character literal closes nothing.
function lex(text: string): { code: string; bare: string } {
  let code = ""
  let bare = ""
  const keep = (s: string) => ((code += s), (bare += s))
  const literal = (s: string, open: number, close: number) => {
    code += s
    bare += s.slice(0, open) + s.slice(open, s.length - close).replace(/[^\n]/g, " ") + s.slice(s.length - close)
  }
  const blank = (s: string) => {
    const b = s.replace(/[^\n]/g, " ")
    code += b
    bare += b
  }
  let i = 0
  while (i < text.length) {
    const word = /\w/.test(text[i - 1] ?? "")
    const raw = /^[bc]?r(#*)"/.exec(text.slice(i, i + 16))
    const quoted = /^[bc]?"/.exec(text.slice(i, i + 2))
    if (raw && !word) {
      const end = text.indexOf(`"${raw[1]}`, i + raw[0].length)
      const stop = end < 0 ? text.length : end + 1 + raw[1].length
      literal(text.slice(i, stop), raw[0].length, 1 + raw[1].length)
      i = stop
    } else if (quoted && !word) {
      let j = i + quoted[0].length
      while (j < text.length && text[j] !== '"') j += text[j] === "\\" ? 2 : 1
      literal(text.slice(i, j + 1), quoted[0].length, 1)
      i = j + 1
    } else if (text[i] === "'" || (text[i] === "b" && text[i + 1] === "'" && !word)) {
      const from = text[i] === "b" ? i + 1 : i
      const lit = /^'(?:\\(?:u\{[0-9a-fA-F]+\}|x[0-9a-fA-F]{2}|.)|[^\\'])'/.exec(text.slice(from, from + 12))
      const stop = lit ? from + lit[0].length : from + 1
      if (lit) literal(text.slice(i, stop), from - i + 1, 1)
      else keep(text.slice(i, stop))
      i = stop
    } else if (text.startsWith("//", i)) {
      const end = text.indexOf("\n", i)
      const stop = end < 0 ? text.length : end
      blank(text.slice(i, stop))
      i = stop
    } else if (text.startsWith("/*", i)) {
      let depth = 0
      let j = i
      do {
        if (text.startsWith("/*", j)) (depth++, (j += 2))
        else if (text.startsWith("*/", j)) (depth--, (j += 2))
        else j++
      } while (depth > 0 && j < text.length)
      blank(text.slice(i, j))
      i = j
    } else {
      keep(text[i])
      i++
    }
  }
  return { code, bare }
}

const pairs: Record<string, string> = { "(": ")", "[": "]", "{": "}" }

// The problems of one file.
function check(file: string, raw: string): string[] {
  const problems: string[] = []
  const { code, bare } = lex(raw)
  // (a) Display or Debug for Private, Display for Text: written impls, under any path.
  for (const m of bare.matchAll(/impl\s*(<[^{]*?>)?\s*(?:[\w:]*::)?(Display|Debug)\s+for\s+(?:&\s*(?:'\w+\s+)?(?:mut\s+)?)?(?:[\w:]*::)?(Private|Text)\b/g)) {
    if (m[3] === "Text" && m[2] === "Debug") continue
    problems.push(`${file}:${at(bare, m.index!)}: ${m[3]} implements ${m[2]}: format! would take it unmarked`)
  }
  // (a) derived: a derive list right above `struct Private`, whatever its visibility.
  for (const m of bare.matchAll(/#\[derive\(([^)]*)\)\]\s*(?:#\[[^\]]*\]\s*)*(?:pub(?:\s*\([^)]*\))?\s+)?struct\s+Private\b/g)) {
    for (const t of ["Debug", "Display"]) if (new RegExp(`\\b${t}\\b`).test(m[1])) problems.push(`${file}:${at(bare, m.index!)}: Private derives ${t}`)
  }
  // (b) IntoPiece for what msg! must refuse.
  for (const m of bare.matchAll(/impl\s*(<[^{]*?>)?\s*(?:[\w:]*::)?IntoPiece\s+for\s+([^{]+?)\s*\{/g)) {
    if (/\bPrivate\b|\bPath(Buf)?\b|path::Display|\bDisplay\s*</.test(m[2])) problems.push(`${file}:${at(bare, m.index!)}: IntoPiece for ${m[2].trim()}: msg! must refuse it`)
  }
  // (c) expose_local outside its list, called or named (`Private::expose_local`, `.map(Private::expose_local)`);
  // its definition in text.rs aside.
  for (const m of bare.matchAll(/\bexpose_local\b/g)) {
    if (file === "src/text.rs" && /\bfn\s+$/.test(bare.slice(Math.max(0, m.index! - 8), m.index!))) continue
    if (!(file in exposeAllowed)) problems.push(`${file}:${at(bare, m.index!)}: expose_local outside the list in scripts/privacy-check.ts`)
  }
  // (d) msg! calls, any delimiter, balanced on the source without literals.
  for (const m of bare.matchAll(/\bmsg!\s*([(\[{])/g)) {
    const [open, close] = [m[1], pairs[m[1]]]
    let depth = 1
    let i = m.index! + m[0].length
    for (; i < bare.length && depth > 0; i++) {
      if (bare[i] === open) depth++
      else if (bare[i] === close) depth--
    }
    const line = at(bare, m.index!)
    const call = bare.slice(m.index!, i)
    if (/\.display\s*\(\s*\)|\.to_string_lossy\s*\(\s*\)|\.to_str\s*\(\s*\)|\bplain\s*\(/.test(call))
      problems.push(`${file}:${line}: msg! formats a path itself; Text::path(p, repo_root) says it with its public form`)
    if (/\.local\s*\(\s*\)|\.into_local\s*\(\s*\)/.test(call)) problems.push(`${file}:${line}: msg! takes a Text's local string, its marks dropped; pass the Text itself`)
    // The format string, plain or raw.
    const lit = /^\s*(?:"((?:[^"\\]|\\.)*)"|r(#*)"([\s\S]*?)"\2)/.exec(code.slice(m.index! + m[0].length, i))
    const fmt = lit ? (lit[1] ?? lit[3] ?? "") : ""
    for (const p of fmt.replace(/\{\{|\}\}/g, "").matchAll(/\{([^}]*)\}/g)) {
      if (!/^\d*(?::[^?*$]*)?$/.test(p[1])) problems.push(`${file}:${line}: msg! placeholder {${p[1]}}: positional arguments only, no capture and no {:?} (marks are lost)`)
    }
  }
  // (e) OS calls that read names, outside their entry points.
  for (const [name, files] of Object.entries(osNames)) {
    if (files.includes(file)) continue
    for (const m of code.matchAll(new RegExp(`\\b${name}\\b`, "g"))) problems.push(`${file}:${at(code, m.index!)}: ${name} outside its entry points (${files.join(", ")})`)
  }
  // (f) text.rs and its submodules are a leaf below model (`super::` in a submodule is text itself).
  if (file === "src/text.rs" || file.startsWith("src/text/")) {
    for (const m of bare.matchAll(/\b(?:crate|super)::[\w:]+/g)) {
      const name = file === "src/text.rs" ? m[0] : m[0].replace(/^super::/, "crate::text::")
      if (!textMayName.some((r) => r.test(name))) problems.push(`${file}:${at(bare, m.index!)}: ${m[0]}: text names only host::home and host::paths of the crate`)
    }
    if (/\bapp::|\bios::logs\b/.test(bare)) problems.push(`${file}: names app or the phone-log module`)
  }
  return problems
}

// --self-test: each of these must be found (the bypasses a review found in an earlier version of this script),
// and the clean lines around them must not hide them by desynchronizing the lexer.
const bad: [string, string][] = [
  ["src/app/x.rs", `const A: &[u8] = br#"a \\" b"#;\nimpl fmt::Debug for super::text::Private<String> {}`],
  ["src/app/x.rs", `#[derive(Clone, Debug)]\npub struct Private<T>(T);`],
  ["src/app/x.rs", `impl<T> std::fmt::Display for crate::text::Private<T> {}`],
  ["src/app/x.rs", `impl fmt::Display for Text {}`],
  ["src/app/x.rs", `impl IntoPiece for std::path::Display<'_> {}`],
  ["src/app/x.rs", `const S: &str = c"x \\" y"; fn f(p: &P) -> String { p.expose_local().clone() }`],
  ["src/app/x.rs", `fn f(p: &Path) -> Text { msg!["in {}", p.display()] }`],
  ["src/app/x.rs", `fn g(p: &Path, c: char) -> Text { msg!("{} {}", if c == ')' { 1 } else { 2 }, p.display()) }`],
  ["src/app/x.rs", `fn h(p: &Path) -> Text { msg!{"in {}", crate::host::paths::plain(p)} }`],
  ["src/app/x.rs", `fn i(e: &Error) -> Text { msg!("stopped: {e:?}") }`],
  ["src/app/x.rs", `fn j(hwnd: isize) { let n = GetWindowTextW(hwnd, 0, 0); }`],
  ["src/app/x.rs", `fn k() { let h = std::env::var("COMPUTERNAME"); }`],
  ["src/text.rs", `fn l() -> String { crate::app::pr::public("") }`],
  // The second review's bypasses.
  ["src/app/x.rs", `fn m(p: &P) -> String { Private::expose_local(p).clone() }`],
  ["src/app/x.rs", `fn n(v: Vec<P>) -> Vec<&String> { v.iter().map(Private::expose_local).collect() }`],
  ["src/app/x.rs", `fn o(t: &Text) -> Text { msg!("x: {}", t.local()) }`],
  ["src/app/x.rs", `fn p(t: Text) -> Text { msg!("x: {}", t.into_local()) }`],
  ["src/app/x.rs", `fn q(p: &Path) -> Text { msg!("x: {}", p.to_str().unwrap_or("")) }`],
  ["src/app/x.rs", `fn r(e: &Error) -> Text { msg!(r#"stopped: {e:?}"#) }`],
  ["src/app/x.rs", `fn s(α: &Error) -> Text { msg!("stopped: {α}") }`],
  ["src/app/x.rs", `impl fmt::Display for &Private<String> {}`],
  ["src/app/x.rs", `#[derive(Clone, Debug)]\npub(crate) struct Private<T>(T);`],
  ["src/app/x.rs", `#[derive(Debug)]\nstruct Private<T>(T);`],
  ["src/text/scrub.rs", `fn t() -> String { crate::app::pr::public("") }`],
]
if (process.argv.includes("--self-test")) {
  const missed = bad.filter(([f, t]) => check(f, t).length === 0)
  const clean = check("src/app/x.rs", `// .expose_local() and msg!("{e:?}", p.display()) in a comment\nconst Q: char = '"'; const R: &str = r#"msg!("{x}", p.display())"#;`)
  for (const [f, t] of missed) console.error(`missed (${f}): ${t}`)
  for (const p of clean) console.error(`flagged a clean line: ${p}`)
  if (missed.length || clean.length) process.exit(1)
  console.log(`privacy self-test: ${bad.length} bypasses found, comments and literals left alone`)
  process.exit(0)
}

for (const { file, text } of sources) problems.push(...check(file, text))
for (const f of Object.keys(exposeAllowed)) {
  if (!sources.some((s) => s.file === f)) problems.push(`scripts/privacy-check.ts: ${f} is listed for expose_local() and does not exist`)
  if (neverExpose.some((r) => r.test(f))) problems.push(`scripts/privacy-check.ts: ${f} composes or publishes text and may never expose a private value`)
  if (!exposeAllowed[f].trim()) problems.push(`scripts/privacy-check.ts: ${f} is listed for expose_local() without a reason`)
}

if (problems.length) {
  console.error(`privacy (SPEC 1 rule 9): private values reach messages only through their public words.\n${problems.join("\n")}`)
  process.exit(1)
}
console.log(`privacy: ${sources.length} files; Private has no Display or Debug, expose_local() in ${Object.keys(exposeAllowed).length} listed files, OS name calls in their entry points, text.rs a leaf`)
