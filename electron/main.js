// Desktop window around the unchanged Bun server: Electron runs Node, and server.ts needs Bun (bun:sqlite, Bun.serve).
import { app, BrowserWindow, Menu, dialog, shell } from "electron";
import { execFileSync, spawn } from "node:child_process";
import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

// Dock and Spotlight start the app with launchd's bare environment (PATH=/usr/bin:/bin:/usr/sbin:/sbin); Linux desktop launchers likewise.
// Load the interactive login shell's environment (PATH from ~/.zshrc, keys from ~/.zshenv) so bun, hooks and MCP servers work as from a terminal.
// The marker skips anything the shell startup prints before env.
function loginShellEnv() {
  try {
    const out = execFileSync(process.env.SHELL || (process.platform === "darwin" ? "/bin/zsh" : "/bin/bash"), ["-ilc", 'printf "\\0__SB__\\0"; env -0'], { encoding: "utf8", timeout: 10_000 });
    const vars = out.split("\0__SB__\0").pop().split("\0").filter(Boolean);
    return Object.fromEntries(vars.map((v) => [v.slice(0, v.indexOf("=")), v.slice(v.indexOf("=") + 1)]));
  } catch (e) {
    console.error(`could not read the shell environment: ${e.message}`);
    return {};
  }
}
// ponytail: Windows has no login shell to read; the app inherits the environment it was started with.
if (app.isPackaged && process.platform !== "win32") Object.assign(process.env, loginShellEnv());

const URL = `http://127.0.0.1:${process.env.PORT ?? 4477}`;
const HOME = process.env.SWITCHBOARD_HOME ?? join(homedir(), ".switchboard");
const mac = process.platform === "darwin";
const linux = process.platform === "linux";
let server, win;

async function waitForServer() {
  for (let i = 0; i < 100; i++) {
    try { await fetch(URL); return; } catch { await new Promise((r) => setTimeout(r, 100)); }
  }
  throw new Error(`switchboard server did not start on ${URL}`);
}

const ICONS = join(import.meta.dirname, "../images/icons");
const page = (js) => win?.webContents.executeJavaScript(js).catch(() => {});

// The window controls take the title bar's colors, now and after each theme change (the page's sb-theme event).
// A reload rejects the pending wait; did-finish-load starts a new one.
const titleBarColors = `(() => { const s = getComputedStyle(document.documentElement);
  return { color: s.getPropertyValue("--bg-void").trim(), symbolColor: s.getPropertyValue("--text-secondary").trim() }; })()`;
async function followTheme() {
  try {
    win.setTitleBarOverlay(await win.webContents.executeJavaScript(titleBarColors));
    for (;;) win.setTitleBarOverlay(await win.webContents.executeJavaScript(
      `new Promise((r) => addEventListener("sb-theme", () => r(${titleBarColors}), { once: true }))`));
  } catch {}
}

// Settings… ⌘, (item 122). The edit role keeps ⌘C/⌘V/⌘A working; a custom menu without it drops them.
function menu() {
  const settings = { label: "Settings…", accelerator: "CmdOrCtrl+,", click: () => page("window.openSettings && window.openSettings()") };
  Menu.setApplicationMenu(Menu.buildFromTemplate([
    mac ? { role: "appMenu", submenu: [{ role: "about" }, { type: "separator" }, settings, { type: "separator" }, { role: "services" }, { type: "separator" },
      { role: "hide" }, { role: "hideOthers" }, { role: "unhide" }, { type: "separator" }, { role: "quit" }] }
      : { label: "File", submenu: [settings, { type: "separator" }, { role: "quit" }] },
    { role: "editMenu" }, { role: "viewMenu" }, { role: "windowMenu" },
  ]));
}

app.whenReady().then(async () => {
  // macOS ignores the BrowserWindow icon and shows the dock icon instead.
  app.dock?.setIcon(join(ICONS, "macos/switchboard-macos-512.png"));
  menu();
  // A server already running on the port wins: this copy exits on EADDRINUSE and the window attaches to the live one.
  server = spawn("bun", ["server.ts"], {
    cwd: join(import.meta.dirname, ".."),
    stdio: ["pipe", "inherit", "inherit"],
    env: { ...process.env, SWITCHBOARD_PARENT: "electron" },
  });
  await waitForServer();
  // The launch token (item 120): the server writes it once its port is bound, also when this window attaches to a running one.
  const token = readFileSync(join(HOME, "token"), "utf8").trim();
  win = new BrowserWindow({
    width: 1400, height: 900, minWidth: 960, minHeight: 600, title: "switchboard", icon: join(ICONS, "flat/switchboard-flat-256.png"),
    backgroundColor: "#0A0813",
    // macOS: our 40px title bar holds the traffic lights (item 121). Linux: it holds Electron's window controls, above
    // its 1px bottom border, and there is no menu bar (the accelerators still work). ponytail: Windows keeps the native frame.
    ...(mac ? { titleBarStyle: "hidden", trafficLightPosition: { x: 14, y: 14 } }
      : linux ? { titleBarStyle: "hidden", titleBarOverlay: { color: "#0A0813", symbolColor: "#BDB9DE", height: 39 } } : {}),
  });
  if (linux) win.webContents.on("did-finish-load", followTheme);
  // Full screen hides the traffic lights, so the page drops the room it leaves for them.
  win.on("enter-full-screen", () => page("document.documentElement.classList.add('fullscreen')"));
  win.on("leave-full-screen", () => page("document.documentElement.classList.remove('fullscreen')"));
  // Markdown links get target="_blank" (index.html); open them in the real browser, not a new Electron window.
  win.webContents.setWindowOpenHandler(({ url }) => { shell.openExternal(url); return { action: "deny" }; });
  win.loadURL(`${URL}/#token=${token}`);
}).catch((e) => { dialog.showErrorBox("switchboard", e.message); app.quit(); });

app.on("window-all-closed", () => app.quit());
// Closing stdin works on Windows too, where kill("SIGTERM") is a hard kill that skips shutdown() in server.ts.
app.on("will-quit", () => server?.stdin.end());
