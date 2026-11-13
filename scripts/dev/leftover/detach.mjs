// A program started directly (no shell) that detaches a 30 ms child and exits at once: a session of its own on
// Unix (`setsid`), a detached process on Windows. Only the run-time net sees it: no text scan of the line can.
import { spawn } from "node:child_process"
import { fileURLToPath } from "node:url"

spawn(process.execPath, [fileURLToPath(new URL("child.mjs", import.meta.url))], { detached: true, stdio: "ignore" }).unref()
