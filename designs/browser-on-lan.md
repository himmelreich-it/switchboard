# Switchboard in a browser on your network

Status: BUILT · 2026-11-05. Setup guide: README, "Open it from another machine". Code: `lan.ts`, the `serve()`
function in `server.ts`, and the pure rules in `lib.ts` (tests in `lib.test.ts`).

## Goal

You run Switchboard on the machine that does the work. In any browser on the network you open
`http://192.168.1.123:4478` (or a name), log in once, and use it as usual. Brave turns it into an app with its own
window and icon.

## How it works (modelled on NZBGet)

One config file, `~/.switchboard/server.json`, written by hand or by `bun server.ts --lan`:

```json
{
  "listen": "0.0.0.0",
  "port": 4478,
  "allow":   ["192.168.1.0/24", "100.64.0.0/10"],
  "trusted": ["192.168.1.50"],
  "names":   ["sb.example.com"],
  "password": "<argon2id hash>",
  "tls": { "cert": "~/.switchboard/desk.pem", "key": "~/.switchboard/desk-key.pem" }
}
```

Without the file nothing changes: only the loopback server on `127.0.0.1:4477` runs, as before.

### Two listeners

The loopback server stays exactly as it was: `127.0.0.1:4477`, the launch token, the agents' `/mcp/app`, and the
Electron window's attach check. With `server.json`, a **second** `Bun.serve` listens on `listen:port` (4478 by
default), with or without TLS, and shares the same request handler (`serve(req, server, lan)`).

The first draft used one listener on `0.0.0.0:4477`. Two are better:
- HTTPS on the network does not break `http://127.0.0.1` for the window app and the agents' MCP URL.
- Binding `127.0.0.1:4477` and `0.0.0.0:4477` at once fails on Linux, so a second port is needed anyway.
- `/mcp/app` is not served on the network listener at all.
- The launch token is not accepted on the network listener; it stays on this machine.

### Who gets in

Each request to the network listener is sorted by the address it comes from (`accessOf`):

| Comes from | What it needs |
|---|---|
| An address in `trusted` | No login: the page asks for a token and gets one. |
| An address in `allow`, or loopback | A login: a passkey or the password. |
| Anything else | Refused (403) before any page or login form is served. |

`allow` defaults to the private ranges (`10/8`, `172.16/12`, `192.168/16`), Tailscale (`100.64/10`) and IPv6 unique
local (`fc00::/7`, which holds Tailscale's IPv6). IPv4-mapped IPv6 addresses count as IPv4. Loopback on the network
listener needs a login like the LAN: other users of the desk must not get in without one.

The Host header must be one of the desk's names (`machineNames`): its host name, `<host>.local`, its interface
addresses, `names` and the DNS names in the certificate, each with the network port. That blocks DNS rebinding,
which matters most for `trusted`: a rebound page would otherwise get a token with no secret. The WebSocket Origin
must be `http(s)://<that Host>`.

### One mechanism under the hood

Every `/api/` request still carries a token in a header (or `?token=` for EventSource and links). On the network it
is a **login token**: 32 random bytes, stored as its SHA-256 in the `logins` table, valid for 30 days after its last
use (`used_at` is written at most once an hour). The page keeps it in `localStorage` like the launch token. The CSRF
rules stay: JSON body required, the token header, the Origin check on WebSockets.

### Login

- **Password**: `bun server.ts --password` stores a `Bun.password` (argon2id) hash in `server.json`. It is read at
  each login, so a change applies without a restart, and it logs out browsers that used the old password. The form
  has a hidden username `switchboard` so password managers save the entry.
- **Passkeys**: `@simplewebauthn/server` (v14). Only over HTTPS with a name, since WebAuthn needs a secure context
  and refuses IP addresses as the site (RP ID). Discoverable credentials, so no username. The page uses the
  browsers' own `PublicKeyCredential.parse…FromJSON` and `toJSON()`, so no browser library is vendored. Each passkey
  belongs to the one name it was made on.
- **One-time link**: `bun server.ts --add-passkey` writes an invite (hash, 10 minutes) and prints
  `https://<name>:<port>/#invite=…`. The page shows "Add a passkey"; saving it uses up the invite and logs that
  browser in.
- **Throttle**: five failed logins (password, passkey or invite) from one address within a minute lock it out for a
  minute. Pending passkey challenges are capped at 1000.

### Settings › Network

Lists the addresses, whether a password and HTTPS are set, the passkeys (Remove) and the logged-in browsers (Log
out, Log out all). Add passkey shows only on an `https://` page with a name. A broken `server.json` keeps the app
off the network but never stops it; the error shows here.

### Small things that come with it

1. A web app manifest: "Switchboard · desk" with the icons in `images/icons/flat/`.
2. The tab title ends with the machine name on a remote page: `(2) switchboard · desk`.
3. VS Code and Open folder are hidden on remote pages and refused by the server there (they open on the desk's
   screen). A Remote-SSH link (`vscode://vscode-remote/ssh-remote+desk/<folder>`) can follow later.
4. The 401 gate becomes the login page on the network listener (`GET /api/login` answers only there).

## Decisions

1. **HTTPS with a name, or plain `http://IP`?** Both. Plain HTTP with the password works out of the box; HTTPS
   (Tailscale, mkcert or an own domain) adds encryption and passkeys. The README warns about plain HTTP.
2. **Username?** No. One user per server; a hidden fixed username satisfies password managers.
3. **Configuration in Settings?** File and command line only. Settings shows the state and manages passkeys and
   logins.
4. **Running as a service** (systemd, launchd) is left out. The window app serves the network too once
   `server.json` exists.

## Later

- A Remote-SSH VS Code link on remote pages.
- Settings fields for `allow`, `trusted` and the password.
- A QR code of the address for phones.
