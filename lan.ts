// Switchboard in a browser on the network (designs/browser-on-lan.md). With ~/.switchboard/server.json a second
// listener serves the same app on the network; the loopback server on 127.0.0.1 stays exactly as it was. Here: that
// file, the logins (a token per browser, stored hashed), the password, the passkeys and the command-line setup.
import type { Database } from "bun:sqlite";
import { X509Certificate, createHash, randomBytes } from "node:crypto";
import { chmodSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { homedir, hostname, networkInterfaces } from "node:os";
import { join } from "node:path";
import {
  generateAuthenticationOptions, generateRegistrationOptions, verifyAuthenticationResponse, verifyRegistrationResponse,
} from "@simplewebauthn/server";
import { DEFAULT_ALLOW, DEFAULT_LAN_PORT, Throttle, accessOf, lanConfigIn, rpIdOf, type Access, type LanConfig } from "./lib";

export const LOGIN_DAYS = 30;          // a login lasts this long after its last use
const INVITE_MINUTES = 10;            // a one-time link of --add-passkey
const CHALLENGE_MS = 5 * 60_000;

export const configFile = (home: string) => join(home, "server.json");
const expandHome = (p: string) => (p === "~" || p.startsWith("~/") ? join(homedir(), p.slice(1)) : p);
const readRaw = (home: string): Record<string, unknown> | null =>
  existsSync(configFile(home)) ? JSON.parse(readFileSync(configFile(home), "utf8")) : null;

// null without server.json: nothing listens on the network. Throws when the file is broken.
export function readLan(home: string, loopbackPort: number): LanConfig | null {
  let raw: Record<string, unknown> | null;
  try { raw = readRaw(home); } catch (e: any) { throw new Error(`server.json is not valid JSON: ${e.message}`); }
  if (!raw) return null;
  try { return lanConfigIn(raw, loopbackPort); } catch (e: any) { throw new Error(`server.json: ${e.message}`); }
}
// Changes some fields and keeps the rest as written. 0600: the file holds the password hash.
function writeLan(home: string, change: Record<string, unknown>) {
  const raw = { ...(readRaw(home) ?? {}), ...change };
  writeFileSync(configFile(home), JSON.stringify(raw, null, 2) + "\n", { mode: 0o600 });
  chmodSync(configFile(home), 0o600);
}
export const tlsFiles = (cfg: LanConfig) =>
  cfg.tls && { cert: readFileSync(expandHome(cfg.tls.cert), "utf8"), key: readFileSync(expandHome(cfg.tls.key), "utf8") };

// The DNS names a certificate is for (tailscale cert, mkcert), so they need no entry in "names".
function certNames(pem: string): string[] {
  try {
    return (new X509Certificate(pem).subjectAltName ?? "").split(/,\s*/).filter((s) => s.startsWith("DNS:")).map((s) => s.slice(4)).filter((s) => !s.includes("*"));
  } catch { return []; }
}
// Every name the Host header may carry: the host name, <host name>.local, this machine's addresses, the names in
// server.json and those of the certificate.
export function machineNames(cfg: LanConfig, certPem: string | null): string[] {
  const h = hostname().toLowerCase();
  const addrs = Object.values(networkInterfaces()).flat().map((i) => i!.address.replace(/%.*$/, "").toLowerCase());
  return [...new Set([h, h.replace(/\.local$/, "") + ".local", ...addrs, ...cfg.names.map((n) => n.toLowerCase()), ...(certPem ? certNames(certPem) : [])])];
}
// The addresses to print at start: the names first, then this machine's IPv4 addresses on the network.
export function lanUrls(cfg: LanConfig, certPem: string | null): string[] {
  const scheme = cfg.tls ? "https" : "http", port = cfg.port === (cfg.tls ? 443 : 80) ? "" : `:${cfg.port}`;
  // Only addresses a browser may come from (a VPN or WSL adapter outside "allow" is left out).
  const v4 = Object.values(networkInterfaces()).flat().filter((i) => i!.family === "IPv4" && !i!.internal).map((i) => i!.address)
    .filter((a) => accessOf(a, cfg.allow, cfg.trusted) !== "refused");
  const all = cfg.listen === "0.0.0.0" || cfg.listen === "::";
  const names = [...cfg.names, ...(certPem ? certNames(certPem) : []), ...(cfg.tls ? [] : [hostname().toLowerCase().replace(/\.local$/, "") + ".local"])]
    .filter((n) => n !== "localhost");
  return [...new Set([...names, ...(all ? v4 : [cfg.listen])])].map((n) => `${scheme}://${n}${port}`);
}

// ---------- logins ----------

const sha = (s: string) => createHash("sha256").update(s).digest("hex");
const now = () => new Date().toISOString();
const daysAgo = (d: number) => new Date(Date.now() - d * 86_400_000).toISOString();

export function issueLogin(db: Database, how: string, address: string, agent: string): string {
  const token = randomBytes(32).toString("hex");
  db.run("INSERT INTO logins (hash, how, address, agent, created_at, used_at) VALUES (?, ?, ?, ?, ?, ?)", [sha(token), how, address, agent.slice(0, 300), now(), now()]);
  return token;
}
// A login token, used within the last 30 days. Its last use is written at most once an hour.
export function loginOk(db: Database, token: string | null): boolean {
  if (!token || !/^[0-9a-f]{64}$/.test(token)) return false;
  const row = db.query("SELECT used_at FROM logins WHERE hash = ?").get(sha(token)) as { used_at: string } | null;
  if (!row || row.used_at < daysAgo(LOGIN_DAYS)) return false;
  if (row.used_at < new Date(Date.now() - 3_600_000).toISOString()) db.run("UPDATE logins SET used_at = ? WHERE hash = ?", [now(), sha(token)]);
  return true;
}
export const pruneLogins = (db: Database) => {
  db.run("DELETE FROM logins WHERE used_at < ?", [daysAgo(LOGIN_DAYS)]);
  db.run("DELETE FROM passkey_invites WHERE expires_at < ?", [now()]);
};

// ---------- passkeys ----------

// One user per server. Its WebAuthn user id is random and kept, so a new passkey replaces the old one of the same
// site in 1Password instead of piling up next to it.
function userId(db: Database): Uint8Array {
  const row = db.query("SELECT value FROM meta WHERE key = 'passkey_user_id'").get() as { value: string } | null;
  if (row) return new Uint8Array(Buffer.from(row.value, "base64url"));
  const id = randomBytes(16);
  db.run("INSERT INTO meta (key, value) VALUES ('passkey_user_id', ?)", [id.toString("base64url")]);
  return new Uint8Array(id);
}
type Challenge = { kind: "add" | "login"; challenge: string; rpId: string; origin: string; invite: string | null; until: number };
const challenges = new Map<string, Challenge>();
function keepChallenge(c: Omit<Challenge, "until">): string {
  for (const [k, v] of challenges) if (v.until < Date.now()) challenges.delete(k);
  // Anyone allowed to reach the login page can ask for challenges; past this many the oldest go.
  for (const k of challenges.keys()) { if (challenges.size < 1000) break; challenges.delete(k); }
  const id = randomBytes(16).toString("hex");
  challenges.set(id, { ...c, until: Date.now() + CHALLENGE_MS });
  return id;
}
function takeChallenge(id: unknown, kind: Challenge["kind"]): Challenge {
  const c = typeof id === "string" ? challenges.get(id) : undefined;
  if (c) challenges.delete(id as string);
  if (!c || c.kind !== kind || c.until < Date.now()) throw Object.assign(new Error("this passkey request expired; try again"), { status: 400 });
  return c;
}
const ALGS = [-8, -7, -257];   // Ed25519, ES256, RS256: what 1Password and platform passkeys make

export type Passkey = { id: string; rp_id: string; name: string; created_at: string; used_at: string | null };
export const passkeysOf = (db: Database) =>
  db.query("SELECT id, rp_id, name, created_at, used_at FROM passkeys ORDER BY created_at").all() as Passkey[];

// ---------- requests to the network listener ----------

// What the server knows of a request: which listener it reached, from where, and with which token.
export type Here = {
  lan: boolean;             // the network listener (false: the loopback one, the window app)
  access: Access;           // by the address (lan only)
  addr: string;
  host: string;             // the Host header, already checked
  secure: boolean;          // HTTPS
  token: string | null;     // the token the request carries
  authed: boolean;          // that token is the launch token or a live login
};
const json = (x: unknown, status = 200) => Response.json(x, { status });
const fail = (error: string, status = 400) => json({ error }, status);
const agentOf = (req: Request) => req.headers.get("user-agent") ?? "";
export const throttle = new Throttle();
// Argon2 takes 64 MB and some 100 ms per check: a burst of password tries waits its turn instead of piling up.
let verifying = 0;

function throttled(here: Here): Response | null {
  const wait = throttle.wait(here.addr, Date.now());
  return wait ? fail(`too many failed logins from ${here.addr}; try again in ${Math.ceil(wait / 1000)} s`, 429) : null;
}
// A passkey belongs to a name (WebAuthn's RP ID) and works only over HTTPS: never for http:// or an IP address.
function passkeySite(here: Here): { rpId: string; origin: string } {
  const rpId = here.lan && here.secure ? rpIdOf(here.host) : null;
  if (!rpId) throw Object.assign(new Error("passkeys need https:// and a name (not an IP address); see the network setup in the README"), { status: 409 });
  return { rpId, origin: `https://${here.host}` };
}

// The login endpoints, which need no token: GET /api/login says how this browser can log in, POST logs in.
// The other routes here need a token and are for the Settings › Network section.
// status: whether the network listener runs, why not, and its addresses.
export async function lanApi(req: Request, path: string, here: Here, db: Database, home: string, loopbackPort: number,
  status: { on: boolean; error: string | null; urls: string[] }): Promise<Response | null> {
  const method = req.method;
  if (!path.startsWith("/api/login") && !path.startsWith("/api/passkeys") && !path.startsWith("/api/logins") && path !== "/api/network") return null;
  const body: any = method === "GET" ? null : await req.json().catch(() => ({}));
  const passkeysHere = () => { try { const { rpId } = passkeySite(here); return !!db.query("SELECT 1 FROM passkeys WHERE rp_id = ?").get(rpId); } catch { return false; } };

  if (path === "/api/login" && method === "GET") {
    if (!here.lan) return fail("not found", 404);
    let cfg: LanConfig | null = null, error: string | null = null;
    try { cfg = readLan(home, loopbackPort); } catch (e: any) { error = `${e.message}; fix it on ${hostname()}`; }
    return json({ trusted: here.access === "trusted", password: !!cfg?.password, passkey: passkeysHere(), secure: here.secure, machine: hostname(), error });
  }
  if (path === "/api/login" && method === "POST") {
    if (!here.lan) return fail("not found", 404);
    if (here.access === "trusted") return json({ token: issueLogin(db, "trusted", here.addr, agentOf(req)) });
    const t = throttled(here);
    if (t) return t;
    // Read at each try, so `bun server.ts --password` applies without a restart.
    const hash = readLan(home, loopbackPort)?.password;
    if (!hash) return fail("no password is set; run `bun server.ts --password` on the server", 409);
    if (verifying >= 4) return fail("busy checking other logins; try again in a moment", 429);
    // Counted as a failure before the check, so parallel tries cannot all pass the throttle; taken back on success.
    const at = Date.now();
    throttle.fail(here.addr, at);
    let ok = false;
    verifying++;
    try { ok = typeof body.password === "string" && (await Bun.password.verify(body.password, hash)); } finally { verifying--; }
    if (!ok) return fail("wrong password", 401);
    throttle.forgive(here.addr, at);
    return json({ token: issueLogin(db, "password", here.addr, agentOf(req)) });
  }
  if (path === "/api/login/passkey/options" && method === "POST") {
    const site = passkeySite(here);
    const t = throttled(here);
    if (t) return t;
    const options = await generateAuthenticationOptions({ rpID: site.rpId, userVerification: "preferred" });
    return json({ id: keepChallenge({ kind: "login", challenge: options.challenge, ...site, invite: null }), options });
  }
  if (path === "/api/login/passkey" && method === "POST") {
    const t = throttled(here);
    if (t) return t;
    const c = takeChallenge(body.id, "login");
    const at = Date.now();
    throttle.fail(here.addr, at);   // as for the password: counted first, taken back on success
    const key = db.query("SELECT * FROM passkeys WHERE id = ? AND rp_id = ?").get(String(body.response?.id ?? ""), c.rpId) as
      { id: string; public_key: Uint8Array; counter: number; transports: string | null } | null;
    let ok = false, counter = 0;
    if (key) {
      try {
        const v = await verifyAuthenticationResponse({
          response: body.response, expectedChallenge: c.challenge, expectedOrigin: c.origin, expectedRPID: c.rpId, requireUserVerification: false,
          credential: { id: key.id, publicKey: new Uint8Array(key.public_key), counter: key.counter, transports: key.transports ? JSON.parse(key.transports) : undefined },
        });
        ok = v.verified;
        counter = v.authenticationInfo.newCounter;
      } catch {}
    }
    if (!ok) return fail(key ? "the passkey did not verify" : "this passkey is not known here; it may have been removed", 401);
    throttle.forgive(here.addr, at);
    db.run("UPDATE passkeys SET counter = ?, used_at = ? WHERE id = ?", [counter, now(), key!.id]);
    return json({ token: issueLogin(db, "passkey", here.addr, agentOf(req)) });
  }
  // Adding a passkey: from a logged-in page, or with the one-time link of --add-passkey (body.invite).
  if (path === "/api/passkeys/options" && method === "POST") {
    const site = passkeySite(here);
    let invite: string | null = null;
    if (!here.authed) {
      const t = throttled(here);
      if (t) return t;
      invite = typeof body.invite === "string" ? sha(body.invite) : null;
      if (!invite || !db.query("SELECT 1 FROM passkey_invites WHERE hash = ? AND expires_at > ?").get(invite, now())) {
        throttle.fail(here.addr, Date.now());
        return fail("this link has expired or was used; run `bun server.ts --add-passkey` again", 401);
      }
    }
    const existing = db.query("SELECT id, transports FROM passkeys WHERE rp_id = ?").all(site.rpId) as { id: string; transports: string | null }[];
    const options = await generateRegistrationOptions({
      rpName: "Switchboard", rpID: site.rpId, userName: "switchboard", userDisplayName: `Switchboard · ${hostname()}`, userID: userId(db) as Uint8Array<ArrayBuffer>,
      attestationType: "none", supportedAlgorithmIDs: ALGS,
      excludeCredentials: existing.map((k) => ({ id: k.id, transports: k.transports ? JSON.parse(k.transports) : undefined })),
      authenticatorSelection: { residentKey: "required", userVerification: "preferred" },
    });
    return json({ id: keepChallenge({ kind: "add", challenge: options.challenge, ...site, invite }), options });
  }
  if (path === "/api/passkeys" && method === "POST") {
    const c = takeChallenge(body.id, "add");
    let info;
    try {
      const v = await verifyRegistrationResponse({ response: body.response, expectedChallenge: c.challenge, expectedOrigin: c.origin, expectedRPID: c.rpId, requireUserVerification: false, supportedAlgorithmIDs: ALGS });
      if (v.verified) info = v.registrationInfo;
    } catch (e: any) { return fail(`the passkey was not saved: ${e.message}`); }
    if (!info) return fail("the passkey was not saved: it did not verify");
    // An invite is used up only now, so a cancelled 1Password dialog can be tried again from the same link.
    if (c.invite && !db.query("DELETE FROM passkey_invites WHERE hash = ? AND expires_at > ? RETURNING hash").get(c.invite, now()))
      return fail("this link has expired or was used; run `bun server.ts --add-passkey` again", 401);
    const name = String(body.name ?? "").trim().slice(0, 80) || `Passkey ${passkeysOf(db).length + 1}`;
    const { credential } = info;
    db.run("INSERT INTO passkeys (id, rp_id, name, public_key, counter, transports, created_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
      [credential.id, c.rpId, name, credential.publicKey, credential.counter, credential.transports ? JSON.stringify(credential.transports) : null, now()]);
    return json({ ok: true, ...(c.invite ? { token: issueLogin(db, "passkey", here.addr, agentOf(req)) } : {}) });
  }

  if (!here.authed) return fail("log in first", 401);
  if (path === "/api/network" && method === "GET") {
    let cfg: LanConfig | null = null, error: string | null = null;
    try { cfg = readLan(home, loopbackPort); } catch (e: any) { error = e.message; }
    error = status.error ?? error;
    const mine = here.token ? sha(here.token) : null;
    let site: string | null = null;
    try { site = passkeySite(here).rpId; } catch {}
    return json({
      config: cfg && { port: cfg.port, listen: cfg.listen, tls: !!cfg.tls, password: !!cfg.password, trusted: cfg.trusted, allow: cfg.allow },
      on: status.on, urls: status.urls, error, file: configFile(home), here: { lan: here.lan, secure: here.secure, site },
      passkeys: passkeysOf(db),
      logins: (db.query("SELECT rowid AS id, hash, how, address, agent, created_at, used_at FROM logins WHERE used_at >= ? ORDER BY used_at DESC").all(daysAgo(LOGIN_DAYS)) as any[])
        .map(({ hash, ...l }) => ({ ...l, current: hash === mine })),
    });
  }
  let m: RegExpMatchArray | null;
  if ((m = path.match(/^\/api\/passkeys\/(.+)$/)) && method === "DELETE") {
    db.run("DELETE FROM passkeys WHERE id = ?", [decodeURIComponent(m[1]!)]);
    return json({});
  }
  if ((m = path.match(/^\/api\/logins\/(\d+)$/)) && method === "DELETE") {
    db.run("DELETE FROM logins WHERE rowid = ?", [Number(m[1]!)]);
    return json({});
  }
  if (path === "/api/logins" && method === "DELETE") {
    db.run("DELETE FROM logins");
    return json({});
  }
  return null;
}

// ---------- the command line ----------

// A line typed without echo (the password). Falls back to a plain line when stdin is not a terminal, so
// `echo … | bun server.ts --password` works in scripts.
async function askHidden(prompt: string): Promise<string> {
  process.stdout.write(prompt);
  const stdin = process.stdin;
  if (!stdin.isTTY) {
    for await (const line of console) return line;
    return "";
  }
  stdin.setRawMode(true);
  stdin.resume();
  return await new Promise<string>((done) => {
    let s = "";
    const on = (buf: Buffer) => {
      for (const ch of buf.toString("utf8")) {
        if (ch === "\r" || ch === "\n") { stdin.off("data", on); stdin.setRawMode(false); stdin.pause(); process.stdout.write("\n"); return done(s); }
        if (ch === "\u0003") { process.stdout.write("\n"); process.exit(130); }
        if (ch === "\u007f" || ch === "\b") s = s.slice(0, -1);
        else if (ch >= " ") s += ch;
      }
    };
    stdin.on("data", on);
  });
}
async function setPassword(db: Database, home: string) {
  const a = await askHidden("New password for the browser login: ");
  if (a.length < 8) throw new Error("use at least 8 characters");
  if ((await askHidden("Again: ")) !== a) throw new Error("the two passwords differ");
  writeLan(home, { password: await Bun.password.hash(a) });
  db.run("DELETE FROM logins WHERE how = 'password'");
  console.log(`Password saved in ${configFile(home)}. Browsers that logged in with the old one log in again.`);
}

export const CLI_HELP = `Switchboard in a browser on the network (see the README):
  bun server.ts --lan            write ${"~"}/.switchboard/server.json if missing, set a password, then start
  bun server.ts --password       set or change the password, then exit
  bun server.ts --add-passkey    print a one-time link (10 minutes) that adds a passkey, then exit
  bun server.ts --logout-all     log every browser out, then exit`;

// Runs a command-line flag. "start" means the server should start afterwards (no flag, or --lan).
export async function lanCli(args: string[], db: Database, home: string, loopbackPort: number): Promise<"start" | "exit"> {
  const flag = args.find((a) => a.startsWith("--"));
  if (!flag) return "start";
  try {
    if (flag === "--lan") {
      if (!existsSync(configFile(home))) {
        writeLan(home, { listen: "0.0.0.0", port: DEFAULT_LAN_PORT, allow: DEFAULT_ALLOW, trusted: [] });
        console.log(`Wrote ${configFile(home)}: listens on port ${DEFAULT_LAN_PORT} for the private ranges and Tailscale.`);
      }
      if (!readLan(home, loopbackPort)?.password) await setPassword(db, home);
      return "start";
    }
    if (flag === "--password") { await setPassword(db, home); return "exit"; }
    if (flag === "--logout-all") {
      const n = (db.query("SELECT COUNT(*) AS n FROM logins").get() as { n: number }).n;
      db.run("DELETE FROM logins");
      console.log(`Logged out ${n} browser${n === 1 ? "" : "s"}.`);
      return "exit";
    }
    if (flag === "--add-passkey") {
      const cfg = readLan(home, loopbackPort);
      if (!cfg) throw new Error("no server.json yet; run `bun server.ts --lan` first");
      if (!cfg.tls) throw new Error("passkeys need HTTPS: add \"tls\" to server.json (see the README)");
      const invite = randomBytes(24).toString("hex");
      db.run("INSERT INTO passkey_invites (hash, expires_at) VALUES (?, ?)", [sha(invite), new Date(Date.now() + INVITE_MINUTES * 60_000).toISOString()]);
      const urls = lanUrls(cfg, tlsFiles(cfg)!.cert).filter((u) => rpIdOf(new URL(u).host));
      if (!urls.length) throw new Error("no name to use: a passkey needs a name, not an IP address; add one to \"names\" in server.json");
      console.log(`Open one of these within ${INVITE_MINUTES} minutes, while the server runs, and save the passkey:`);
      for (const u of urls) console.log(`  ${u}/#invite=${invite}`);
      return "exit";
    }
    console.log(CLI_HELP);
    return "exit";
  } catch (e: any) {
    console.error(`switchboard: ${e.message}`);
    process.exit(1);
  }
}
