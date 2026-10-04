// Desktop window around the unchanged Bun server: Electron runs Node, and server.ts needs Bun (bun:sqlite, Bun.serve).
import { app, BrowserWindow, dialog, shell } from "electron";
import { spawn } from "node:child_process";
import { join } from "node:path";

const URL = `http://127.0.0.1:${process.env.PORT ?? 4477}`;
let server;

async function waitForServer() {
  for (let i = 0; i < 100; i++) {
    try { await fetch(URL); return; } catch { await new Promise((r) => setTimeout(r, 100)); }
  }
  throw new Error(`switchboard server did not start on ${URL}`);
}

const ICONS = join(import.meta.dirname, "../images/icons");

app.whenReady().then(async () => {
  // macOS ignores the BrowserWindow icon and shows the dock icon instead.
  app.dock?.setIcon(join(ICONS, "switchboard-512.png"));
  // A server already running on the port wins: this copy exits on EADDRINUSE and the window attaches to the live one.
  server = spawn("bun", ["server.ts"], {
    cwd: join(import.meta.dirname, ".."),
    stdio: ["pipe", "inherit", "inherit"],
    env: { ...process.env, SWITCHBOARD_PARENT: "electron" },
  });
  await waitForServer();
  const win = new BrowserWindow({ width: 1400, height: 900, title: "switchboard", icon: join(ICONS, "switchboard-256.png") });
  // Markdown links get target="_blank" (index.html); open them in the real browser, not a new Electron window.
  win.webContents.setWindowOpenHandler(({ url }) => { shell.openExternal(url); return { action: "deny" }; });
  win.loadURL(URL);
}).catch((e) => { dialog.showErrorBox("switchboard", e.message); app.quit(); });

app.on("window-all-closed", () => app.quit());
// Closing stdin works on Windows too, where kill("SIGTERM") is a hard kill that skips shutdown() in server.ts.
app.on("will-quit", () => server?.stdin.end());
