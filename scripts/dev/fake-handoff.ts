// A fake launcher (dev only, not shipped): starts the fake tray app as a process of its own and exits at
// once, as an app's launcher that hands over does. The tray app writes to the launcher's output.
const app = Bun.spawn([process.execPath, import.meta.dir + "/fake-tray.ts"], { stdio: ["ignore", "inherit", "inherit"] })
app.unref()
