// Rule 9 (SPEC 1): what belongs to the machine is marked where a message is composed (src/text.rs), and the
// compiler keeps it so only while these hold. Fails when:
//   (a) `Private` gains `Display` or `Debug` (derived or written), or `Text` gains `Display`: `format!` would take
//       either whole, its private parts unmarked;
//   (b) `IntoPiece` (what `msg!` takes) is implemented for `Private`, a path or a path's `Display`;
//   (c) `.expose_local()` is used outside the files listed below, each with why;
//   (d) a `msg!` call formats a path itself (`.display()`, `.to_string_lossy()`): `Text::path` says it.
// It reads files only (no build). `bun scripts/dev/private-text.ts` proves the compiler half.
//
//   bun scripts/privacy-check.ts
import { readdirSync, readFileSync, statSync } from "node:fs"
import { join, relative } from "node:path"

const root = join(import.meta.dir, "..")

// Where `expose_local()` may hand out a private value, and why: what never becomes text (an OS call, a wire
// protocol, a comparison with another of the machine's names).
const exposeAllowed: Record<string, string> = {}

const walk = (d: string): string[] =>
  readdirSync(d).flatMap((n) => {
    const f = join(d, n)
    return statSync(f).isDirectory() ? walk(f) : f.endsWith(".rs") ? [f] : []
  })
const sources = walk(join(root, "src")).map((f) => ({ file: relative(root, f).replace(/\\/g, "/"), text: readFileSync(f, "utf8").replace(/\r\n/g, "\n") }))
const problems: string[] = []
const at = (text: string, i: number) => text.slice(0, i).split("\n").length

// Comments blanked (line, doc and block comments), strings and character literals kept: a sentence about
// `expose_local()` is no use of it, and a `//` inside a string is no comment.
function code(text: string): string {
  let out = ""
  let i = 0
  while (i < text.length) {
    const rest = text.slice(i, i + 3)
    const raw = /^r(#*)"/.exec(text.slice(i, i + 12))
    if (raw && !/\w/.test(text[i - 1] ?? "")) {
      const end = text.indexOf(`"${raw[1]}`, i + raw[0].length)
      const stop = end < 0 ? text.length : end + 1 + raw[1].length
      out += text.slice(i, stop)
      i = stop
    } else if (text[i] === '"') {
      let j = i + 1
      while (j < text.length && text[j] !== '"') j += text[j] === "\\" ? 2 : 1
      out += text.slice(i, j + 1)
      i = j + 1
    } else if (text[i] === "'") {
      const lit = /^'(?:\\(?:u\{[0-9a-fA-F]+\}|x[0-9a-fA-F]{2}|.)|[^\\'])'/.exec(text.slice(i, i + 12))
      const n = lit ? lit[0].length : 1
      out += text.slice(i, i + n)
      i += n
    } else if (rest.startsWith("//")) {
      const end = text.indexOf("\n", i)
      const stop = end < 0 ? text.length : end
      out += " ".repeat(stop - i)
      i = stop
    } else if (rest.startsWith("/*")) {
      let depth = 0
      let j = i
      do {
        if (text.startsWith("/*", j)) (depth++, (j += 2))
        else if (text.startsWith("*/", j)) (depth--, (j += 2))
        else j++
      } while (depth > 0 && j < text.length)
      out += text.slice(i, j).replace(/[^\n]/g, " ")
      i = j
    } else {
      out += text[i]
      i++
    }
  }
  return out
}

for (const { file, text: raw } of sources) {
  const text = code(raw)
  // (a) Display or Debug for Private, Display for Text: written impls, anywhere in src.
  for (const m of text.matchAll(/impl\s*(<[^{]*?>)?\s*(?:(?:std::|core::)?fmt::)?(Display|Debug)\s+for\s+(?:(?:crate::)?text::)?(Private|Text)\b/g)) {
    if (m[3] === "Text" && m[2] === "Debug") continue
    problems.push(`${file}:${at(text, m.index!)}: ${m[3]} implements ${m[2]}: format! would take it unmarked`)
  }
  // (a) derived: a derive list right above `struct Private`.
  for (const m of text.matchAll(/#\[derive\(([^)]*)\)\]\s*(?:#\[[^\]]*\]\s*)*pub\s+struct\s+Private\b/g)) {
    for (const t of ["Debug", "Display"]) if (new RegExp(`\\b${t}\\b`).test(m[1])) problems.push(`${file}:${at(text, m.index!)}: Private derives ${t}`)
  }
  // (b) IntoPiece for what msg! must refuse.
  for (const m of text.matchAll(/impl\s*(<[^{]*?>)?\s*(?:(?:crate::)?text::)?IntoPiece\s+for\s+([^{]+?)\s*\{/g)) {
    if (/\bPrivate\b|\bPath(Buf)?\b|path::Display|\bDisplay\s*</.test(m[2])) problems.push(`${file}:${at(text, m.index!)}: IntoPiece for ${m[2].trim()}: msg! must refuse it`)
  }
  // (c) expose_local outside its list (its definition aside).
  for (const m of text.matchAll(/\.expose_local\s*\(/g)) {
    if (!(file in exposeAllowed)) problems.push(`${file}:${at(text, m.index!)}: expose_local() outside the list in scripts/privacy-check.ts`)
  }
  // (d) msg! calls: their arguments, parentheses balanced, strings skipped.
  for (const m of text.matchAll(/\bmsg!\s*\(/g)) {
    let depth = 1
    let i = m.index! + m[0].length
    for (; i < text.length && depth > 0; i++) {
      const c = text[i]
      if (c === '"') {
        for (i++; i < text.length && text[i] !== '"'; i++) if (text[i] === "\\") i++
      } else if (c === "(") depth++
      else if (c === ")") depth--
    }
    const call = text.slice(m.index!, i)
    if (/\.display\(\)|\.to_string_lossy\(\)/.test(call)) problems.push(`${file}:${at(text, m.index!)}: msg! formats a path itself; Text::path(p, repo_root) says it with its public form`)
  }
}
for (const f of Object.keys(exposeAllowed)) if (!sources.some((s) => s.file === f)) problems.push(`scripts/privacy-check.ts: ${f} is listed for expose_local() and does not exist`)

if (problems.length) {
  console.error(`privacy (SPEC 1 rule 9): private values reach messages only through their public words.\n${problems.join("\n")}`)
  process.exit(1)
}
console.log(`privacy: ${sources.length} files; Private has no Display or Debug, expose_local() in ${Object.keys(exposeAllowed).length} listed files`)
