// An Electron tray-style app for strobe's windowless guards (dev only): its one BrowserWindow loads the
// page named on its command line and stays hidden (`show: false`, as a tray app's is until asked
// for), unless STROBE_DEV_SHOW=1 shows it. Run as `electron scripts/dev/tray/electron <url>`.
const { app, BrowserWindow } = require("electron")

// STROBE_DEV_HIDE=1: the hidden window is not painted while hidden (`paintWhenInitiallyHidden: false`), so its page
// reports visibilityState hidden, as Electron documents; by default a never-shown window's page reports visible.
app.whenReady().then(() => {
  const hide = process.env.STROBE_DEV_HIDE === "1"
  const win = new BrowserWindow({ width: 480, height: 320, show: process.env.STROBE_DEV_SHOW === "1", paintWhenInitiallyHidden: !hide })
  win.loadURL(process.argv.find((a) => /^(https?|file):/.test(a)) ?? "about:blank")
  win.webContents.on("did-finish-load", () =>
    win.webContents.executeJavaScript("document.visibilityState").then((v) => console.log(`window ${win.isVisible() ? "shown" : "hidden"}, page ${v}`)),
  )
})
// A tray app keeps running with no window open.
app.on("window-all-closed", () => {})
