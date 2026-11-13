#!/usr/bin/env node
// Runs this platform's strobe binary, from the @hona/strobe-<platform>-<arch> package npm installed beside this
// one (an optional dependency: each machine installs only its own). STROBE_BINARY points at another build.
import { spawn } from "node:child_process"
import { createRequire } from "node:module"
import { dirname, join } from "node:path"

const target = `${process.platform}-${process.arch}`
let exe = process.env.STROBE_BINARY
if (!exe) {
  try {
    const require = createRequire(import.meta.url)
    const dir = dirname(require.resolve(`@hona/strobe-${target}/package.json`))
    exe = join(dir, process.platform === "win32" ? "strobe.exe" : "strobe")
  } catch {
    console.error(`strobe: no build for ${target} is installed (@hona/strobe-${target}). strobe ships for win32-x64, win32-arm64, darwin-arm64, darwin-x64, linux-x64 and linux-arm64.`)
    process.exit(1)
  }
}
const child = spawn(exe, process.argv.slice(2), { stdio: "inherit" })
// A stop (Ctrl-C, SIGINT, SIGTERM) is strobe's to carry out: it ends what it launched and says what it
// left before it exits, so this waits for it rather than handing the shell back while it still stops.
// A terminal's Ctrl-C or `timeout` reaches strobe directly; one sent to this process alone is passed on
// (on Unix: strobe takes a repeat within 300 ms as the same stop). Windows' console sends Ctrl-C to
// every process on it, and Node would end the child rather than signal it, so nothing is passed there.
for (const sig of ["SIGINT", "SIGTERM"]) {
  process.on(sig, () => {
    if (process.platform !== "win32") child.kill(sig)
  })
}
child.on("error", (e) => {
  console.error(`strobe: ${e.message}`)
  process.exit(1)
})
child.on("exit", (code, signal) => {
  if (signal) {
    process.removeAllListeners(signal)
    process.kill(process.pid, signal)
  }
  process.exit(code ?? 1)
})
