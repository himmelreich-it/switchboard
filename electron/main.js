// Desktop window around the unchanged Bun server: Electron runs Node, and server.ts needs Bun (bun:sqlite, Bun.serve).
import { app, BrowserWindow, Menu, dialog, shell } from "electron";
import { execFile, spawn } from "node:child_process";
import { readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

// Dock and Spotlight start the app with launchd's bare environment (PATH=/usr/bin:/bin:/usr/sbin:/sbin); Linux desktop launchers likewise.
// Load the interactive login shell's environment (PATH from ~/.zshrc, keys from ~/.zshenv) so bun, hooks and MCP servers work as from a terminal.
// The marker skips anything the shell startup prints before env. Read in the background from the first line on, so the
// window opens while the shell starts; only the server waits for it.
function loginShellEnv() {
  return new Promise((done) => execFile(process.env.SHELL || (process.platform === "darwin" ? "/bin/zsh" : "/bin/bash"), ["-ilc", 'printf "\\0__SB__\\0"; env -0'], { encoding: "utf8", timeout: 10_000, maxBuffer: 16 << 20 }, (e, out) => {
    if (e) { console.error(`could not read the shell environment: ${e.message}`); return done({}); }
    const vars = out.split("\0__SB__\0").pop().split("\0").filter(Boolean);
    done(Object.fromEntries(vars.map((v) => [v.slice(0, v.indexOf("=")), v.slice(v.indexOf("=") + 1)])));
  }));
}
// Windows has no login shell to read; the app inherits the environment it was started with (PATH from the user's settings).
const shellEnv = app.isPackaged && process.platform !== "win32" ? loginShellEnv() : Promise.resolve({});

// Read after the shell environment is in, which may set PORT or SWITCHBOARD_HOME.
const URL = () => `http://127.0.0.1:${process.env.PORT ?? 4477}`;
const HOME = () => process.env.SWITCHBOARD_HOME ?? join(homedir(), ".switchboard");
const mac = process.platform === "darwin";
const linux = process.platform === "linux";
const windows = process.platform === "win32";
let server, win;

async function waitForServer() {
  for (let i = 0; i < 400; i++) {
    try { await fetch(URL()); return; } catch { await new Promise((r) => setTimeout(r, 25)); }
  }
  throw new Error(`switchboard server did not start on ${URL()}`);
}

// A beta build (install-local with --beta / -Beta) shares ~/.switchboard and the port with the regular app, so its
// TODOs and history are the same; it installs next to it under its own name and icon. The two cannot run at once: a
// second server on the same data would end the first one's runs and fire its schedules twice.
const CHANNEL = JSON.parse(readFileSync(join(import.meta.dirname, "../package.json"), "utf8")).channel ?? "stable";
const beta = CHANNEL === "beta";
const NAME = beta ? "Switchboard Beta" : "Switchboard";
const ICONS = join(import.meta.dirname, beta ? "../images/beta-icons" : "../images/icons");
const FLAT = beta ? "flat/switchboard-beta" : "flat/switchboard";

// The channel of the server already on the port, null when the port is free. A server from before channels sends no header: stable.
async function runningChannel() {
  try { return (await fetch(URL(), { signal: AbortSignal.timeout(2000) })).headers.get("x-switchboard-channel") ?? "stable"; } catch { return null; }
}
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

// Settings… ⌘, (item 122), Alt+, on Linux and Windows like the page's shortcuts. The edit role keeps ⌘C/⌘V/⌘A working; a custom menu without it drops them.
function menu() {
  const settings = { label: "Settings…", accelerator: mac ? "Cmd+," : "Alt+,", click: () => page("window.openSettings && window.openSettings()") };
  Menu.setApplicationMenu(Menu.buildFromTemplate([
    mac ? { role: "appMenu", submenu: [{ role: "about" }, { type: "separator" }, settings, { type: "separator" }, { role: "services" }, { type: "separator" },
      { role: "hide" }, { role: "hideOthers" }, { role: "unhide" }, { type: "separator" }, { role: "quit" }] }
      : { label: "File", submenu: [settings, { type: "separator" }, { role: "quit" }] },
    { role: "editMenu" }, { role: "viewMenu" }, { role: "windowMenu" },
  ]));
}

app.whenReady().then(async () => {
  // macOS ignores the BrowserWindow icon and shows the dock icon instead.
  app.dock?.setIcon(join(ICONS, beta ? "macos/switchboard-beta-macos-512.png" : "macos/switchboard-macos-512.png"));
  menu();
  // The other build on the same data: refuse before opening anything.
  const other = await runningChannel();
  if (other && other !== CHANNEL) throw new Error(`${other === "beta" ? "Switchboard Beta" : "Switchboard"} is running on the same data. Quit it first, then start ${NAME}.`);
  // The window opens at once, in the page's background color; the page loads once the server answers.
  win = new BrowserWindow({
    width: 1400, height: 900, minWidth: 960, minHeight: 600, title: "switchboard",
    // Windows shows the window icon in the taskbar; its .ico fills the frame like other apps, the padded PNG did not.
    icon: join(ICONS, windows ? `${FLAT}.ico` : `${FLAT}-flat-256.png`),
    backgroundColor: "#0A0813",
    // macOS: our 40px title bar holds the traffic lights (item 121). Linux and Windows: it holds Electron's window
    // controls, above its 1px bottom border, and there is no menu bar (the accelerators still work).
    ...(mac ? { titleBarStyle: "hidden", trafficLightPosition: { x: 14, y: 14 } }
      : { titleBarStyle: "hidden", titleBarOverlay: { color: "#0A0813", symbolColor: "#BDB9DE", height: 39 } }),
  });
  if (linux || windows) win.webContents.on("did-finish-load", followTheme);
  // Full screen hides the traffic lights, so the page drops the room it leaves for them.
  win.on("enter-full-screen", () => page("document.documentElement.classList.add('fullscreen')"));
  win.on("leave-full-screen", () => page("document.documentElement.classList.remove('fullscreen')"));
  // Markdown links get target="_blank" (index.html); open them in the real browser, not a new Electron window.
  win.webContents.setWindowOpenHandler(({ url }) => { shell.openExternal(url); return { action: "deny" }; });
  Object.assign(process.env, await shellEnv);
  // A server already running on the port wins: this copy exits on EADDRINUSE and the window attaches to the live one.
  server = spawn("bun", ["server.ts"], {
    cwd: join(import.meta.dirname, ".."),
    stdio: ["pipe", "inherit", "inherit"],
    env: { ...process.env, SWITCHBOARD_PARENT: "electron", SWITCHBOARD_CHANNEL: CHANNEL },
    // bun.exe is a console program: launched from the GUI app on Windows it opens its own console window without this.
    windowsHide: true,
  });
  await waitForServer();
  // The launch token (item 120): the server writes it once its port is bound, also when this window attaches to a running one.
  const token = readFileSync(join(HOME(), "token"), "utf8").trim();
  win.loadURL(`${URL()}/#token=${token}`);
}).catch((e) => { dialog.showErrorBox(NAME, e.message); app.quit(); });

app.on("window-all-closed", () => app.quit());
// Closing stdin works on Windows too, where kill("SIGTERM") is a hard kill that skips shutdown() in server.ts.
app.on("will-quit", () => server?.stdin.end());
