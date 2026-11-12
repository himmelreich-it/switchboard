# switchboard

Local web app where every TODO runs as one or more Claude Code sessions: capture TODOs across topics in a backlog,
start each one when ready, steer it with short chat messages, start a fresh session when a step is done, browse and
edit the files of its worktree, and open terminals in it. A pinned operator session answers questions about all
TODOs and acts only when told. Spec: `designs/feature-spec-switchboard.md`.

```sh
bun install
bun server.ts        # prints http://127.0.0.1:4477/#token=… (PORT to change); open that link
bun server.ts --lan  # also serve browsers on other machines, with a login (see "Open it from another machine")
bun run app          # same app in its own window (Electron); closing it stops running agents and terminals like Ctrl-C
bun test             # pure helpers in lib.ts
bun scripts/bench.ts # server start, list and transcript load times on a throwaway home (args: todos events)
bun run vendor       # rebuild vendor/ (CodeMirror, xterm.js, marked, DOMPurify, fonts) after changing one of them
bun run install-local  # build Switchboard.app (no installer) and copy it to /Applications, for Spotlight and the Dock
bun run app:linux            # Linux: `app` without Chromium's sandbox (see below)
bun run install-local:linux  # Linux: build for this machine's arch, install to ~/.local/share/switchboard with a launcher
bun run install-local:win    # Windows: build for this machine's arch (x64 or ARM64), install to %LOCALAPPDATA%\Programs\Switchboard
```

The API needs the launch token the server picks at each start. `bun server.ts` prints a link with it, and the window
app reads it from `~/.switchboard/token`. After a server restart, open the new link. Everything the page loads is in
`vendor/`, so the app works offline. Terminals need Bun 1.3.5 or newer and are not available on Windows yet.

`install-local` quits a running Switchboard first, which stops its running agents and terminals. The installed app
runs a copy of the code: run `install-local` again to update it. It still needs `bun` installed; it reads PATH and
other variables from your login shell (`zsh -ilc`), since apps started from the Dock or Spotlight do not get them.

`install-local:linux` does the same for your user, without sudo: the app goes to `~/.local/share/switchboard`, with a
desktop entry and icons (app grid and dock) and `~/.local/bin/switchboard` for terminals. On Linux the login shell
is `$SHELL -ilc` (bash reads `~/.profile`, so bun must be on PATH there). Both Linux commands run with `--no-sandbox`:
Ubuntu 24.04+ blocks the user namespaces Chromium's sandbox needs, and the fallback wants a root-owned
`chrome-sandbox`. The window only loads its own `127.0.0.1` page and opens links in your browser.

`install-local:win` installs the same way for your user, without admin rights: the app goes to
`%LOCALAPPDATA%\Programs\Switchboard`, with a Start menu shortcut (search for Switchboard, pin it to the taskbar).
There is no login shell on Windows: the app inherits your user environment, where the bun installer puts `bun` on
PATH. On Windows and Linux the app's title bar holds the window controls, like VS Code; there is no menu bar, but
Alt+, still opens Settings.

To try a branch before it lands, install it as a beta from its worktree: `bun run install-local:win:beta`,
`install-local:linux:beta` or `install-local:beta` (macOS). That installs "Switchboard Beta" next to the regular app,
with its own folder, shortcut and icon, and runs `bun install` first in a fresh worktree. Both use the same data in
`~/.switchboard/` and the same port, so the TODOs and their history are the same. Only one runs at a time: a second
server on the same data would end the first one's runs, so the other app refuses to start until you quit the running
one. Appearance settings are kept per app.

Needs a Claude Code login. Agents run with your normal Claude Code settings, in auto permission mode. A TODO can run
on Codex instead (the agent select, or a topic's default agent); it uses your Codex login and `~/.codex/config.toml`,
and starts in Codex's auto review mode. Topics are
set up in Settings › Topics (the gear, or ⌘,). A topic is one folder, a git repository or not: agents start
there, and only a git repository gets worktrees, branches and pull requests. In the code and the database a topic is
still a project. Shortcuts written with ⌘ use Alt on Linux and Windows.
Data lives in `~/.switchboard/` (`SWITCHBOARD_HOME` to change): `db.sqlite`, `token` while the server runs, and
`data/<todo-id>/` for the agents' files. On first start, topics are imported from
`~/.auto-coder/db.sqlite`.

## Open it from another machine

Switchboard can serve the same app to browsers on your network: you run it on the machine that does the work (the
"desk"), and open it from a laptop, a tablet or another desktop. Agents, git and terminals keep running on the desk;
the other machines only show the page. Nothing changes until you create `~/.switchboard/server.json`. The window
app and `http://127.0.0.1:4477` keep working as before, and the network gets its own port, 4478 by default.

### Quick start: plain HTTP on your home network

On the desk:

```sh
bun server.ts --lan     # writes ~/.switchboard/server.json, asks for a password, then starts as usual
```

It prints the addresses to open, for example `http://192.168.1.123:4478` and `http://desk.local:4478`. Open one
on the other machine and log in with the password. Brave, Chrome and 1Password offer to save it. The window app
reads the same `server.json`, so once the file exists the window app serves the network too.

Let the port through the desk's firewall:

- **Windows**: the first start shows "Windows Defender Firewall has blocked some features of bun". Allow it on
  **private** networks. If you dismissed the dialog, run this in an admin PowerShell:
  `New-NetFirewallRule -DisplayName "Switchboard" -Direction Inbound -Protocol TCP -LocalPort 4478 -Action Allow -Profile Private`.
  Your Wi-Fi must be set to a private network (Settings › Network › Wi-Fi › the network › Private).
- **macOS**: allow incoming connections for `bun` when asked (System Settings › Network › Firewall).
- **Linux** with ufw: `sudo ufw allow from 192.168.0.0/16 to any port 4478 proto tcp`.

> **Security warning:** over plain `http://`, the password and the login cross the network unencrypted. Anyone on the
> same Wi-Fi or LAN can capture them, and with them gets agent control and a shell on the desk. Plain HTTP is fine on
> your own home network. On an office, shared or café network, use HTTPS (below).

### HTTPS and passkeys

A passkey (1Password, or the browser's own) logs you in without typing anything, and HTTPS encrypts the connection.
Browsers allow passkeys only on `https://` with a **name**, never on `http://` or an IP address. Pick one way to get
a name with a certificate:

| Way | Name | Certificate | Other machines need |
|---|---|---|---|
| Tailscale | `desk.<tailnet>.ts.net` | `tailscale cert` | Tailscale, nothing else |
| mkcert | `desk.local` | `mkcert` | mkcert's root certificate installed once |
| Your own domain | `sb.example.com` pointing at the desk | Let's Encrypt (DNS challenge) | nothing |

**Tailscale.** In the Tailscale admin console, turn on MagicDNS and HTTPS certificates (DNS page). Then on the desk:

```sh
cd ~/.switchboard
tailscale cert desk.<tailnet>.ts.net      # writes desk.<tailnet>.ts.net.crt and .key here
```

Add the files to `server.json` and restart Switchboard:

```json
"tls": { "cert": "~/.switchboard/desk.<tailnet>.ts.net.crt", "key": "~/.switchboard/desk.<tailnet>.ts.net.key" }
```

Open `https://desk.<tailnet>.ts.net:4478`. Tailscale certificates last 90 days: run `tailscale cert` again before
then, and restart Switchboard.

If another machine cannot connect but the desk itself can, check Tailscale's "block incoming connections" on the
desk (`tailscale debug prefs` shows `ShieldsUp`; `tailscale set --shields-up=false` turns it off, which lets your
other tailnet devices reach the desk). `ping` proves nothing here: Windows does not answer it by default. Test with
`curl https://desk.<tailnet>.ts.net:4478/api/login` from the other machine.

**mkcert.** On the desk, run `mkcert -install` and then `mkcert desk.local 192.168.1.123` (your address) in
`~/.switchboard`, and put the two files it names in `tls` as above. Each other machine must trust mkcert's root
certificate: copy `rootCA.pem` from the folder `mkcert -CAROOT` prints, and import it there (Windows: double-click,
Install Certificate › Local Machine › Trusted Root Certification Authorities; macOS: Keychain Access › System ›
Always Trust). `desk.local` works on macOS and Windows; Linux machines need Avahi (`nss-mdns`).

Switchboard accepts every name in the certificate, plus the desk's host name and addresses. Other names go in
`"names"`.

**Adding a passkey.** Open Switchboard by its `https://` name and log in with the password. Then go to
Settings › Network, enter a name, and press **Add passkey**; 1Password or the browser saves it. Each passkey works
for the one address it was made on. For a desk without a screen, `bun server.ts --add-passkey` prints a link that
adds a passkey without logging in first; it works once, within 10 minutes. Once you have a passkey you can drop the
password: remove `"password"` from `server.json`.

### Install it as an app

Brave and Chrome: menu › **Install Switchboard** (or Cast, save and share › Install page as app). Over plain
`http://` the browser may offer only **Create shortcut** › "Open as window". The app is named
"Switchboard · desk", and so is the tab title, so you can tell machines apart.

The VS Code and Open folder buttons are hidden in a browser on another machine, since they would open on the desk's
screen.

### server.json

```json
{
  "listen": "0.0.0.0",
  "port": 4478,
  "allow": ["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16", "100.64.0.0/10", "fc00::/7"],
  "trusted": [],
  "names": [],
  "password": "<set with bun server.ts --password>",
  "tls": { "cert": "<path>", "key": "<path>" }
}
```

| Field | Meaning | Default |
|---|---|---|
| `listen` | The address to listen on: `0.0.0.0` for all, `::` for IPv6 too, or one address (your Tailscale IP, say). | `0.0.0.0` |
| `port` | The network port. It must differ from the window app's 4477. | `4478` |
| `allow` | Addresses and ranges that may log in. Everything else is refused before any page is served. Tailscale's `100.64.0.0/10` is also the range some internet providers use for their own network (carrier-grade NAT): without Tailscale, leave it out. | private ranges and Tailscale |
| `trusted` | Addresses that need no login at all. | none |
| `names` | Extra names the desk is reached by (a DNS name, say). | none |
| `password` | The password's hash (argon2id). | none |
| `tls` | The certificate and key (PEM files) for HTTPS. `~` is your home folder. | none: plain HTTP |

Each request is sorted by its address: from `trusted` it needs no login; from `allow` (or from the desk itself) it
needs the password or a passkey; anything else is refused. A login lasts 30 days from its last use. The password
applies at once; restart Switchboard after the other changes. If `server.json` is broken, Switchboard still starts,
without the network, and Settings › Network shows the error.

> **Security warning:** a `trusted` address logs in with no secret at all. If the router hands that address to another
> device, or a guest joins the network, that device gets the same access, including a shell on the desk. Give trusted
> machines a fixed address in the router, or leave `trusted` empty and use a passkey.

### Commands

```sh
bun server.ts --lan            # write server.json if missing, set a password if none, then start
bun server.ts --password       # set or change the password (logs out browsers that used the old one), then exit
bun server.ts --add-passkey    # print a one-time link (10 minutes) that adds a passkey, then exit
bun server.ts --logout-all     # log every browser out, then exit
```

The last three work while Switchboard runs. Settings › Network lists the addresses, the passkeys and the logged-in browsers,
each with a Remove or Log out button.
