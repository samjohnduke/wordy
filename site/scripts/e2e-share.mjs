// End-to-end check of sharing against a running `wrangler dev` (port 8787):
// an owner invites a reader by email, the invitee creates an account from
// the link (headless Chromium, see e2e-auth.mjs --invite) and links a device,
// the room lets them pull but not push, a role change reaches the open
// socket, and removal closes it and refuses the next connect.
//
//   pnpm wrangler dev --port 8787   (in another terminal)
//   pnpm test:e2e:share
import { spawnSync } from "node:child_process";
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

const BASE = process.env.E2E_BASE ?? "http://localhost:8787";
const WS_BASE = BASE.replace(/^http/, "ws");
const MAIL_DIR = join(process.cwd(), ".wrangler", "tmp", "email");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const assert = (cond, msg) => {
  if (!cond) throw new Error(`assertion failed: ${msg}`);
};
const step = (msg) => console.log(`• ${msg}`);

async function api(path, { method = "POST", body, token, headers = {} } = {}) {
  const res = await fetch(BASE + path, {
    method,
    headers: {
      "content-type": "application/json",
      origin: BASE,
      ...(token ? { authorization: `Bearer ${token}` } : {}),
      ...headers,
    },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const text = await res.text();
  let json = null;
  try {
    json = JSON.parse(text);
  } catch {
    // not json
  }
  return { status: res.status, json, text };
}

function newestMail(sinceMs) {
  let best = null;
  const walk = (dir) => {
    for (const name of readdirSync(dir)) {
      const p = join(dir, name);
      const st = statSync(p);
      if (st.isDirectory()) walk(p);
      else if (p.includes("email-text") && st.mtimeMs >= sinceMs && (!best || st.mtimeMs > best.mtime)) {
        best = { path: p, mtime: st.mtimeMs };
      }
    }
  };
  walk(MAIL_DIR);
  return best ? readFileSync(best.path, "utf8") : null;
}

async function waitForMail(sinceMs, pattern) {
  for (let i = 0; i < 50; i++) {
    const mail = newestMail(sinceMs);
    const hit = mail?.match(pattern)?.[0];
    if (hit) return hit;
    await sleep(100);
  }
  throw new Error(`no email matching ${pattern} under ${MAIL_DIR}`);
}

async function deviceCode() {
  const r = await api("/api/auth/device/code", { body: { client_id: "wordy-app" } });
  assert(r.status === 200, `device code: ${r.status} ${r.text}`);
  return r.json;
}

async function pollToken(device_code) {
  let r;
  for (let i = 0; i < 20; i++) {
    r = await api("/api/auth/device/token", {
      body: { grant_type: "urn:ietf:params:oauth:grant-type:device_code", device_code, client_id: "wordy-app" },
    });
    if (r.status === 200) break;
    await sleep(1000);
  }
  assert(r.status === 200 && r.json?.access_token, `token: ${r.status} ${r.text}`);
  return r.json.access_token;
}

async function register(token, name) {
  const r = await api("/api/devices", { token, body: { name, platform: "linux", appVersion: "0.1.0" } });
  assert(r.status === 200, `register: ${r.status} ${r.text}`);
  return r.json.device.id;
}

/** A fresh account with a linked device, made through the browser script. */
async function linkDevice(name, inviteUrl) {
  const { user_code, device_code } = await deviceCode();
  const args = ["scripts/e2e-auth.mjs"];
  if (inviteUrl) args.push("--invite", inviteUrl);
  args.push("--approve", user_code);
  const run = spawnSync("node", args, { stdio: ["ignore", "pipe", "inherit"], env: { ...process.env, E2E_BASE: BASE } });
  assert(run.status === 0, `browser script failed: ${run.stdout}`);
  const email = String(run.stdout).match(/approved as (\S+)/)?.[1];
  const token = await pollToken(device_code);
  return { token, email, deviceId: await register(token, name) };
}

class Socket {
  constructor(ws) {
    this.ws = ws;
    this.queue = [];
    this.waiters = [];
    ws.binaryType = "arraybuffer";
    ws.addEventListener("message", (ev) => {
      let msg;
      if (typeof ev.data === "string") msg = JSON.parse(ev.data);
      else {
        const buf = Buffer.from(ev.data);
        msg = { t: "update", seq: Number(buf.readBigUInt64BE(0)), bytes: buf.subarray(8) };
      }
      this.push(msg);
    });
    ws.addEventListener("close", (ev) => this.push({ t: "closed", code: ev.code, reason: ev.reason }));
  }
  static async open(projectId, token) {
    const ws = new WebSocket(`${WS_BASE}/parties/project-room/${projectId}`, {
      headers: { authorization: `Bearer ${token}` },
    });
    const sock = new Socket(ws);
    await new Promise((resolve, reject) => {
      ws.addEventListener("open", resolve);
      ws.addEventListener("error", (e) => reject(new Error(`socket error: ${e.message ?? e}`)));
      ws.addEventListener("close", (ev) => reject(new Error(`closed ${ev.code} ${ev.reason}`)));
    });
    return sock;
  }
  push(msg) {
    const w = this.waiters.shift();
    if (w) w(msg);
    else this.queue.push(msg);
  }
  next(timeoutMs = 10000) {
    if (this.queue.length) return Promise.resolve(this.queue.shift());
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("timed out waiting for a message")), timeoutMs);
      this.waiters.push((m) => {
        clearTimeout(timer);
        resolve(m);
      });
    });
  }
  async until(t, timeoutMs) {
    for (;;) {
      const m = await this.next(timeoutMs);
      if (m.t === t) return m;
      if (m.t === "closed" || m.t === "error") throw new Error(`got ${JSON.stringify(m)} while waiting for ${t}`);
    }
  }
  hello(since, extra = {}) {
    this.ws.send(JSON.stringify({ t: "hello", version: 1, since, ...extra }));
  }
  close() {
    this.ws.close();
  }
}

/** Open a socket that is expected to be refused at the upgrade; resolve with the close event. */
function refused(projectId, token) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(`${WS_BASE}/parties/project-room/${projectId}`, {
      headers: { authorization: `Bearer ${token}` },
    });
    ws.addEventListener("open", () => reject(new Error("socket opened but should have been refused")));
    ws.addEventListener("error", () => {});
    ws.addEventListener("close", (ev) => resolve(ev));
  });
}

const ulid = () => {
  const chars = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
  let s = "";
  for (let i = 0; i < 26; i++) s += chars[Math.floor(Math.random() * 32)];
  return s;
};

step("the owner links a device and syncs a project");
const owner = await linkDevice("Owner box");
const project = ulid();
const so = await Socket.open(project, owner.token);
so.hello(0, { name: "Shared novel" });
let m = await so.until("welcome");
assert(m.role === "owner", JSON.stringify(m));
await so.until("synced");
so.ws.send(Buffer.from("owner wrote this"));
m = await so.until("ack");
assert(m.seq === 1, "first update");
let r;

step("only the owner can invite, and only with a sane address and role");
const outsider = await linkDevice("Outsider");
r = await api(`/api/projects/${project}/invitations`, { token: outsider.token, body: { email: "x@example.test", role: "reader" } });
assert(r.status === 403, `outsider invite: ${r.status} ${r.text}`);
r = await api(`/api/projects/${project}/invitations`, { token: owner.token, body: { email: "not-an-email", role: "reader" } });
assert(r.status === 400, `bad email: ${r.status}`);
r = await api(`/api/projects/${project}/invitations`, { token: owner.token, body: { email: "x@example.test", role: "owner" } });
assert(r.status === 400, `owner role: ${r.status}`);
r = await api(`/api/projects/${project}/invitations`, { token: owner.token, body: { email: owner.email, role: "reader" } });
assert(r.status === 400, `self invite: ${r.status} ${r.text}`);

step("invite a reader by email; the invitation is listed and can be withdrawn and re-sent");
const readerEmail = `e2e-reader-${Date.now()}@example.test`;
let t0 = Date.now();
r = await api(`/api/projects/${project}/invitations`, { token: owner.token, body: { email: readerEmail, role: "reader" } });
assert(r.status === 200 && r.json.invitation.role === "reader", `invite: ${r.status} ${r.text}`);
const firstLink = await waitForMail(t0 - 1000, /https?:\/\/\S+\/invite\/\S+/);
r = await api(`/api/projects/${project}/members`, { method: "GET", token: owner.token });
assert(r.status === 200 && r.json.invitations.length === 1 && r.json.members.length === 1, `members: ${r.text}`);
r = await api(`/api/projects/${project}/invitations/${r.json.invitations[0].id}`, { method: "DELETE", token: owner.token });
assert(r.status === 204, `withdraw: ${r.status}`);
t0 = Date.now();
r = await api(`/api/projects/${project}/invitations`, { token: owner.token, body: { email: readerEmail, role: "reader" } });
assert(r.status === 200, `re-invite: ${r.status} ${r.text}`);
const link = await waitForMail(t0 - 1000, /https?:\/\/\S+\/invite\/\S+/);
assert(link !== firstLink, "re-sent invitation reused the token");

step("a withdrawn link is dead; a stranger sees the offer to create an account");
let res = await fetch(firstLink);
assert(res.status === 200 && (await res.text()).includes("not valid"), "withdrawn link still works");
res = await fetch(link);
assert((await res.text()).includes("Create my account"), "invite page did not offer sign-up");

step("the invitee creates an account from the link, joins, and links a device");
const reader = await linkDevice("Reader laptop", link);
assert(reader.email === readerEmail, `joined as ${reader.email}`);
r = await api("/api/projects", { method: "GET", token: reader.token });
const listed = r.json.projects.find((p) => p.id === project);
assert(listed && listed.role === "reader" && listed.owner === false && listed.name === "Shared novel", `reader's list: ${r.text}`);
res = await fetch(link);
assert((await res.text()).includes("already used"), "used link not reported as used");

step("the reader pulls the log but cannot push, add words or announce assets");
const sr = await Socket.open(project, reader.token);
sr.hello(0);
m = await sr.until("welcome");
assert(m.role === "reader" && m.head === 1, JSON.stringify(m));
m = await sr.until("update");
assert(m.seq === 1 && m.bytes.toString() === "owner wrote this", "reader did not get the log");
await sr.until("synced");
sr.ws.send(JSON.stringify({ t: "dictionary", words: ["Mordor"] }));
sr.ws.send(Buffer.from("reader push"));
m = await sr.until("error");
assert(/readers cannot push/.test(m.message), m.message);
m = await sr.next();
assert(m.t === "closed" && m.code === 4403, JSON.stringify(m));
r = await api(`/api/projects/${project}/members`, { method: "GET", token: reader.token });
assert(r.status === 200 && r.json.invitations.length === 0 && r.json.members.length === 2, `reader's view: ${r.text}`);
const readerId = r.json.members.find((x) => x.role === "reader").userId;

step("the reader cannot change roles or remove the owner");
r = await api(`/api/projects/${project}/members/${readerId}`, { method: "PATCH", token: reader.token, body: { role: "editor" } });
assert(r.status === 403, `self promote: ${r.status}`);
const ownerId = r.json ? (await api(`/api/projects/${project}/members`, { method: "GET", token: reader.token })).json.members.find((x) => x.role === "owner").userId : null;
r = await api(`/api/projects/${project}/members/${ownerId}`, { method: "DELETE", token: reader.token });
assert(r.status === 403, `remove owner: ${r.status}`);

step("promotion to editor reaches the open socket; the reader can now push");
const sr2 = await Socket.open(project, reader.token);
sr2.hello(1);
await sr2.until("synced");
r = await api(`/api/projects/${project}/members/${readerId}`, { method: "PATCH", token: owner.token, body: { role: "editor" } });
assert(r.status === 200, `promote: ${r.status} ${r.text}`);
m = await sr2.until("role");
assert(m.role === "editor", JSON.stringify(m));
sr2.ws.send(Buffer.from("editor wrote this"));
m = await sr2.until("ack");
assert(m.seq === 2, "editor push");
// The owner's queue still holds the echo of its own first update and presence notices.
do m = await so.until("update");
while (m.seq < 2);
assert(m.seq === 2 && m.bytes.toString() === "editor wrote this", "owner did not get the editor's update");
r = await api(`/api/projects/${project}/members/${ownerId}`, { method: "PATCH", token: owner.token, body: { role: "reader" } });
assert(r.status === 404, `demote owner: ${r.status}`);

step("removal closes the socket and the next connect is refused");
r = await api(`/api/projects/${project}/members/${readerId}`, { method: "DELETE", token: owner.token });
assert(r.status === 204, `remove: ${r.status} ${r.text}`);
m = await sr2.until("error");
assert(/no longer/.test(m.message), m.message);
m = await sr2.next();
assert(m.t === "closed" && m.code === 4403, JSON.stringify(m));
const ev = await refused(project, reader.token);
assert(ev.code !== 1000, "removed member could reconnect");
r = await api("/api/projects", { method: "GET", token: reader.token });
assert(!r.json.projects.some((p) => p.id === project), "still listed after removal");
r = await api(`/api/projects/${project}/members`, { method: "GET", token: reader.token });
assert(r.status === 403, `removed member can still list: ${r.status}`);

step("a member can leave on their own");
t0 = Date.now();
r = await api(`/api/projects/${project}/invitations`, { token: owner.token, body: { email: readerEmail, role: "editor" } });
assert(r.status === 200, `invite again: ${r.status} ${r.text}`);
const link2 = await waitForMail(t0 - 1000, /https?:\/\/\S+\/invite\/\S+/);
res = await fetch(link2, { headers: { authorization: `Bearer ${reader.token}` } });
assert((await res.text()).includes("Join as editor"), "existing member's invite page");
res = await fetch(link2, {
  method: "POST",
  headers: { authorization: `Bearer ${reader.token}`, "content-type": "application/x-www-form-urlencoded", origin: BASE },
  body: "action=accept",
});
assert(res.status === 200 && (await res.text()).includes("You are in"), `accept: ${res.status}`);
r = await api(`/api/projects/${project}/members/${readerId}`, { method: "DELETE", token: reader.token });
assert(r.status === 204, `leave: ${r.status} ${r.text}`);
r = await api("/api/projects", { method: "GET", token: reader.token });
assert(!r.json.projects.some((p) => p.id === project), "still listed after leaving");

so.close();
console.log(`\nall good: ${owner.email} shared ${project} with ${reader.email}`);
process.exit(0);
