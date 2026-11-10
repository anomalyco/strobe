// The script() target of startup.bench.ts (dev only, not shipped): at its import it starts a child that spins 4 s (a
// slow start that keeps working after the host is ready), and `wait` sleeps.
import { join } from "node:path"

const bin = process.env.KNOWN_BIN ?? join(import.meta.dir, process.platform === "win32" ? "known.exe" : "known")
Bun.spawn([bin, "spin", "4000"], { stdout: "ignore", stderr: "ignore" })

export async function wait(ms: number) {
  await Bun.sleep(ms)
  return true
}
