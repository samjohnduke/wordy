// End-to-end check of a project room against a running `wrangler dev` (port
// 8787): two devices of one account share a room over websockets, large blobs
// go through HTTP and R2, a snapshot becomes the new base, dictionary words
// and asset announcements fan out, and readers/strangers are refused.
//
//   pnpm wrangler dev --port 8787   (in another terminal)
//   pnpm test:e2e:room
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { request as httpRequest } from "node:http";

const BASE = process.env.E2E_BASE ?? "http://localhost:8787";
const WS_BASE = BASE.replace(/^http/, "ws");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const assert = (cond, msg) => {
  if (!cond) throw new Error(`assertion failed: ${msg}`);
};
const step = (msg) => console.log(`• ${msg}`);

async function api(path, { method = "POST", body, token, headers = {}, raw } = {}) {
  const res = await fetch(BASE + path, {
    method,
    headers: {
      "content-type": raw ? "application/octet-stream" : "application/json",
      origin: BASE,
      ...(token ? { authorization: `Bearer ${token}` } : {}),
      ...headers,
    },
    body: raw ?? (body === undefined ? undefined : JSON.stringify(body)),
  });
  const buf = Buffer.from(await res.arrayBuffer());
  let json = null;
  try {
    json = JSON.parse(buf.toString("utf8"));
  } catch {
    // not json
  }
  return { status: res.status, json, text: buf.toString("utf8"), bytes: buf };
}

/** A linked device: a fresh account approved through the browser script. */
async function linkDevice(name) {
  let r = await api("/api/auth/device/code", { body: { client_id: "wordy-app" } });
  assert(r.status === 200, `device code: ${r.status} ${r.text}`);
  const { user_code, device_code } = r.json;
  const approve = spawnSync("node", ["scripts/e2e-auth.mjs", "--approve", user_code], {
    stdio: ["ignore", "pipe", "inherit"],
    env: { ...process.env, E2E_BASE: BASE },
  });
  assert(approve.status === 0, `approve script failed: ${approve.stdout}`);
  const email = String(approve.stdout).match(/approved as (\S+)/)?.[1];
  for (let i = 0; i < 20; i++) {
    r = await api("/api/auth/device/token", {
      body: { grant_type: "urn:ietf:params:oauth:grant-type:device_code", device_code, client_id: "wordy-app" },
    });
    if (r.status === 200) break;
    await sleep(1000);
  }
  assert(r.status === 200 && r.json?.access_token, `token: ${r.status} ${r.text}`);
  const token = r.json.access_token;
  r = await api("/api/devices", { token, body: { name, platform: "linux", appVersion: "0.1.0" } });
  assert(r.status === 200, `register: ${r.status} ${r.text}`);
  return { token, email, deviceId: r.json.device.id };
}

/** A websocket with a queue of parsed messages. */
class Socket {
  constructor(ws) {
    this.ws = ws;
    this.queue = [];
    this.waiters = [];
    this.closed = null;
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
    ws.addEventListener("close", (ev) => {
      this.closed = { code: ev.code, reason: ev.reason };
      this.push({ t: "closed", code: ev.code, reason: ev.reason });
    });
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
  /** Skip messages until one of type `t` arrives. */
  async until(t, timeoutMs) {
    for (;;) {
      const m = await this.next(timeoutMs);
      if (m.t === t) return m;
      if (m.t === "closed" || m.t === "error") throw new Error(`got ${JSON.stringify(m)} while waiting for ${t}`);
    }
  }
  send(obj) {
    this.ws.send(JSON.stringify(obj));
  }
  hello(since, extra = {}) {
    this.send({ t: "hello", version: 1, since, ...extra });
  }
  close() {
    this.ws.close();
  }
}

/**
 * The HTTP status a websocket upgrade gets. Node's WebSocket hides it, so send
 * the upgrade by hand and read the response line.
 */
function upgradeStatus(projectId, token) {
  return new Promise((resolve, reject) => {
    const url = new URL(`${BASE}/parties/project-room/${projectId}`);
    const req = httpRequest(
      {
        host: url.hostname,
        port: url.port,
        path: url.pathname,
        headers: {
          connection: "Upgrade",
          upgrade: "websocket",
          "sec-websocket-version": "13",
          "sec-websocket-key": "dGhlIHNhbXBsZSBub25jZQ==",
          ...(token ? { authorization: `Bearer ${token}` } : {}),
        },
      },
      (res) => {
        res.resume();
        resolve(res.statusCode);
      },
    );
    req.on("upgrade", (res, socket) => {
      socket.destroy();
      resolve(res.statusCode);
    });
    req.on("error", reject);
    req.end();
  });
}

const ulid = () => {
  const chars = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
  let s = "";
  for (let i = 0; i < 26; i++) s += chars[Math.floor(Math.random() * 32)];
  return s;
};
const sha256 = (buf) => createHash("sha256").update(buf).digest("hex");

step("link two devices of one account, and a stranger");
const a = await linkDevice("Room box A");
let r;
const stranger = await linkDevice("Stranger");
const project = ulid();

step("no token, bad project id and a stranger are refused");
assert((await upgradeStatus(project, null)) === 401, "no token");
assert((await upgradeStatus(project, "nope")) === 401, "bad token");
assert((await upgradeStatus("not-a-ulid", a.token)) === 404, "bad project id");

step("first connect claims the project and gets an empty welcome");
const sa = await Socket.open(project, a.token);
sa.hello(0, { name: "Room novel", dictionary: ["Gondor"] });
let m = await sa.until("welcome");
assert(m.head === 0 && m.base === 0 && m.from === 0 && m.reset === false && m.role === "owner", JSON.stringify(m));
assert(m.dictionary.includes("Gondor"), "hello dictionary not stored");
m = await sa.until("synced");
assert(m.head === 0, "synced head");
m = await sa.until("presence");
assert(m.devices.length === 1 && m.devices[0].name === "Room box A", JSON.stringify(m));

step("the stranger cannot open it");
assert((await upgradeStatus(project, stranger.token)) === 403, "stranger");
r = await api("/api/projects", { method: "GET", token: a.token });
assert(r.status === 200 && r.json.projects.some((p) => p.id === project && p.name === "Room novel" && p.role === "owner"), r.text);
r = await api("/api/projects", { method: "GET", token: stranger.token });
assert(r.status === 200 && !r.json.projects.some((p) => p.id === project), "stranger lists the project");

step("push an inline update: ack to the sender, echoed with its seq");
const u1 = Buffer.from("update one");
sa.ws.send(u1);
m = await sa.until("ack");
assert(m.seq === 1, `ack seq ${m.seq}`);
m = await sa.until("update");
assert(m.seq === 1 && m.bytes.equals(u1), "echo of update one");

step("a second device of the same account replays from the start");
// Register a second device on A's account: a second device flow approved by the same
// account is what the app does; here we reuse A's token for a second registration.
r = await api("/api/auth/device/code", { body: { client_id: "wordy-app" } });
const b = await linkDeviceOnSameAccount(a, r.json);
const sb = await Socket.open(project, b.token);
sb.hello(0);
m = await sb.until("welcome");
assert(m.head === 1 && m.from === 1 && m.log_count === 1 && m.log_bytes === u1.length, JSON.stringify(m));
m = await sb.until("update");
assert(m.seq === 1 && m.bytes.equals(u1), "replayed update one");
m = await sb.until("synced");
assert(m.head === 1, "synced at 1");
m = await sa.until("presence");
assert(m.devices.length === 2, `presence after B: ${JSON.stringify(m)}`);
await sb.until("presence");

step("B pushes; A receives it live");
const u2 = Buffer.from("update two from B");
sb.ws.send(u2);
m = await sb.until("ack");
assert(m.seq === 2, "ack 2");
m = await sa.until("update");
assert(m.seq === 2 && m.bytes.equals(u2), "A got update two");
await sb.until("update");

step("dictionary and asset announcements fan out to the other device only");
sb.send({ t: "dictionary", words: ["Rohan", "Gondor", " "] });
m = await sa.until("dictionary");
assert(m.words.length === 1 && m.words[0] === "Rohan", JSON.stringify(m));
const file = Buffer.from("a tiny attachment");
const sha = sha256(file);
r = await api(`/api/projects/${project}/assets/${sha}`, { method: "PUT", token: b.token, raw: file });
assert(r.status === 200 && r.json.existed === false, `asset put: ${r.status} ${r.text}`);
r = await api(`/api/projects/${project}/assets/${sha}`, { method: "PUT", token: b.token, raw: file });
assert(r.status === 200 && r.json.existed === true, "asset dedupe");
sb.send({ t: "asset", path: "assets/tiny.txt", sha256: sha, size: file.length });
m = await sa.until("asset");
assert(m.path === "assets/tiny.txt" && m.sha256 === sha, JSON.stringify(m));
r = await api(`/api/projects/${project}/assets/${sha}`, { method: "GET", token: a.token });
assert(r.status === 200 && r.bytes.equals(file), "asset get");
r = await api(`/api/projects/${project}/assets/${sha}`, { method: "GET", token: stranger.token });
assert(r.status === 403, `stranger asset get: ${r.status}`);

step("a big blob goes over HTTP and is announced as `blob`");
const big = Buffer.alloc(600 * 1024, 7);
r = await api(`/api/projects/${project}/updates`, { method: "PUT", token: a.token, raw: big });
assert(r.status === 200 && r.json.seq === 3 && r.json.base === 0, `big put: ${r.status} ${r.text}`);
m = await sa.until("ack");
assert(m.seq === 3, "http ack to the sender comes first");
m = await sa.until("blob");
assert(m.seq === 3 && m.size === big.length, JSON.stringify(m));
m = await sb.until("blob");
assert(m.seq === 3, "B told about the blob");
r = await api(`/api/projects/${project}/updates/3`, { method: "GET", token: b.token });
assert(r.status === 200 && r.bytes.equals(big), `blob get: ${r.status} ${r.bytes.length}`);
r = await api(`/api/projects/${project}/updates/1`, { method: "GET", token: b.token });
assert(r.status === 200 && r.bytes.equals(u1), "inline update over http");

step("a snapshot taken at the head becomes the base; older rows vanish");
const snap = Buffer.from("snapshot after 3");
r = await api(`/api/projects/${project}/updates`, {
  method: "PUT",
  token: a.token,
  raw: snap,
  headers: { "x-wordy-snapshot-at": "3" },
});
assert(r.status === 200 && r.json.seq === 4 && r.json.base === 4, `snapshot put: ${r.text}`);
m = await sb.until("base_moved");
assert(m.base === 4, "base moved");
await sb.until("blob");
r = await api(`/api/projects/${project}/updates/1`, { method: "GET", token: b.token });
assert(r.status === 404, "compacted update still served");
r = await api(`/api/projects/${project}/updates/3`, { method: "GET", token: b.token });
assert(r.status === 404, "compacted R2 blob still served");

step("a stale snapshot (head moved on) is kept as a normal update");
sb.ws.send(Buffer.from("update five"));
await sb.until("ack");
r = await api(`/api/projects/${project}/updates`, {
  method: "PUT",
  token: a.token,
  raw: Buffer.from("stale snapshot"),
  headers: { "x-wordy-snapshot-at": "4" },
});
assert(r.status === 200 && r.json.seq === 6 && r.json.base === 4, `stale snapshot: ${r.text}`);

step("reconnecting behind the base replays from the base; ahead of the head resets");
const sc = await Socket.open(project, b.token);
sc.hello(1);
m = await sc.until("welcome");
assert(m.head === 6 && m.base === 4 && m.from === 4 && m.reset === false, JSON.stringify(m));
m = await sc.until("blob");
assert(m.seq === 4, "replay starts at the base snapshot");
m = await sc.until("update");
assert(m.seq === 5, "then the update after it");
m = await sc.until("blob");
assert(m.seq === 6, "then the stale snapshot");
await sc.until("synced");
sc.close();
const sd = await Socket.open(project, b.token);
sd.hello(99);
m = await sd.until("welcome");
assert(m.reset === true && m.from === 0 && m.head === 6, JSON.stringify(m));
await sd.until("synced");
sd.close();

step("a device that is up to date gets nothing replayed");
const se = await Socket.open(project, b.token);
se.hello(6);
m = await se.until("welcome");
assert(m.from === 0 && m.reset === false, JSON.stringify(m));
m = await se.until("synced");
se.close();

step("protocol mismatch and junk are closed with a reason");
const sf = await Socket.open(project, b.token);
sf.hello(0, { version: 99 });
m = await sf.until("error");
assert(/protocol/.test(m.message), m.message);
m = await sf.next();
assert(m.t === "closed" && m.code === 4426, JSON.stringify(m));
const sg = await Socket.open(project, b.token);
sg.ws.send(Buffer.from("before hello"));
m = await sg.until("error");
assert(/hello/.test(m.message), m.message);

step("a revoked device cannot reconnect");
r = await api(`/api/devices/${b.deviceId}`, { method: "DELETE", token: a.token });
assert(r.status === 204, `revoke: ${r.status} ${r.text}`);
assert((await upgradeStatus(project, b.token)) === 401, "revoked device");
sa.close();
sb.close();
console.log(`\nall good for ${a.email} (${project})`);
process.exit(0);

/** Approve a second device code with an already-linked device's session. */
async function linkDeviceOnSameAccount(first, code) {
  // The web approval page needs a browser session; the bearer session of a
  // device also counts as signed in, so approve through the API directly.
  let r = await api(`/api/auth/device?user_code=${encodeURIComponent(code.user_code)}`, {
    method: "GET",
    token: first.token,
  });
  assert(r.status === 200, `verify via bearer: ${r.status} ${r.text}`);
  r = await api("/api/auth/device/approve", { token: first.token, body: { userCode: code.user_code } });
  assert(r.status === 200, `approve via bearer: ${r.status} ${r.text}`);
  for (let i = 0; i < 20; i++) {
    r = await api("/api/auth/device/token", {
      body: {
        grant_type: "urn:ietf:params:oauth:grant-type:device_code",
        device_code: code.device_code,
        client_id: "wordy-app",
      },
    });
    if (r.status === 200) break;
    await sleep(1000);
  }
  assert(r.status === 200 && r.json?.access_token, `token B: ${r.status} ${r.text}`);
  const token = r.json.access_token;
  r = await api("/api/devices", { token, body: { name: "Room box B", platform: "macos", appVersion: "0.1.0" } });
  assert(r.status === 200, `register B: ${r.status} ${r.text}`);
  return { token, deviceId: r.json.device.id };
}
