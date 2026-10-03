// End-to-end check of the account flows against a running `wrangler dev` (port 8787 by
// default): sign-up by email link, first passkey, sign-out, passkey sign-in, device
// approval, device registration and revocation. Uses headless Chromium over the DevTools
// protocol with a virtual authenticator, so no real passkey is touched.
//
//   pnpm wrangler dev --port 8787   (in another terminal)
//   pnpm test:e2e
import { spawn } from "node:child_process";
import { mkdtempSync, readdirSync, readFileSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

// `node scripts/e2e-auth.mjs --approve <user_code>` only creates a fresh
// account with a passkey and approves that device code, for the Rust
// client's test (crates/wordy-sync/tests/cloud.rs). With `--invite <url>`
// the account is created from an invitation link instead (and joins the
// project), for scripts/e2e-share.mjs; `--approve` may follow it.
const args = process.argv.slice(2);
const flag = (name) => (args.includes(name) ? args[args.indexOf(name) + 1] : null);
const APPROVE = flag("--approve");
const INVITE = flag("--invite");
const BASE = process.env.E2E_BASE ?? "http://localhost:8787";
const CHROME = process.env.CHROME ?? "chromium";
let EMAIL = `e2e-${Date.now()}@example.test`;
const MAIL_DIR = join(process.cwd(), ".wrangler", "tmp", "email");

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const assert = (cond, msg) => {
  if (!cond) throw new Error(`assertion failed: ${msg}`);
};
const step = (msg) => console.log(`• ${msg}`);

// --- tiny DevTools client ---------------------------------------------------------
class Cdp {
  constructor(ws) {
    this.ws = ws;
    this.id = 0;
    this.pending = new Map();
    ws.addEventListener("message", (ev) => {
      const msg = JSON.parse(ev.data);
      if (msg.id && this.pending.has(msg.id)) {
        const { resolve, reject } = this.pending.get(msg.id);
        this.pending.delete(msg.id);
        msg.error ? reject(new Error(`${msg.error.message} (${msg.error.data ?? ""})`)) : resolve(msg.result);
      }
    });
  }
  send(method, params = {}, sessionId) {
    const id = ++this.id;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.ws.send(JSON.stringify({ id, method, params, sessionId }));
    });
  }
}

async function launchChrome() {
  const dir = mkdtempSync(join(tmpdir(), "wordy-e2e-"));
  const port = 9000 + Math.floor(Math.random() * 1000);
  const child = spawn(
    CHROME,
    [
      "--headless=new",
      "--no-sandbox",
      "--disable-gpu",
      `--remote-debugging-port=${port}`,
      `--user-data-dir=${dir}`,
      "--no-first-run",
      "about:blank",
    ],
    { stdio: "ignore" },
  );
  let version;
  for (let i = 0; i < 100; i++) {
    try {
      version = await (await fetch(`http://127.0.0.1:${port}/json/version`)).json();
      break;
    } catch {
      await sleep(100);
    }
  }
  if (!version) throw new Error("chromium did not start");
  const ws = new WebSocket(version.webSocketDebuggerUrl);
  await new Promise((r) => ws.addEventListener("open", r));
  return { child, cdp: new Cdp(ws) };
}

class Page {
  constructor(cdp, sessionId) {
    this.cdp = cdp;
    this.sid = sessionId;
  }
  send(method, params) {
    return this.cdp.send(method, params, this.sid);
  }
  async goto(url) {
    await this.send("Page.navigate", { url });
    await this.waitFor("document.readyState === 'complete'");
  }
  async eval(expression) {
    const r = await this.send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.text + ": " + JSON.stringify(r.exceptionDetails.exception?.description));
    return r.result.value;
  }
  async waitFor(expression, timeoutMs = 15000) {
    const until = Date.now() + timeoutMs;
    while (Date.now() < until) {
      try {
        if (await this.eval(expression)) return;
      } catch {
        // mid-navigation; try again
      }
      await sleep(100);
    }
    const where = await this.eval("location.href").catch(() => "?");
    const err = await this.eval("document.querySelector('main')?.innerText.slice(0, 600) ?? ''").catch(() => "");
    throw new Error(`timed out waiting for: ${expression}\n  at ${where}${err ? `\n  page says: ${err}` : ""}`);
  }
  async path() {
    return new URL(await this.eval("location.href")).pathname;
  }
}

// --- helpers ------------------------------------------------------------------------
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

// --- the walk-through -----------------------------------------------------------------
const { child, cdp } = await launchChrome();
try {
  const { targetId } = await cdp.send("Target.createTarget", { url: "about:blank" });
  const { sessionId } = await cdp.send("Target.attachToTarget", { targetId, flatten: true });
  const page = new Page(cdp, sessionId);
  await page.send("Page.enable");
  await page.send("Runtime.enable");
  await page.send("WebAuthn.enable");
  const { authenticatorId } = await page.send("WebAuthn.addVirtualAuthenticator", {
    options: {
      protocol: "ctap2",
      transport: "internal",
      hasResidentKey: true,
      hasUserVerification: true,
      isUserVerified: true,
      automaticPresenceSimulation: true,
    },
  });

  const t0 = Date.now();
  let r;
  if (INVITE) {
    step("open the invitation as a stranger: it offers to create the account");
    await page.goto(INVITE);
    await page.waitFor(`document.querySelector('button[type=submit]')?.innerText.includes('Create my account')`);
    EMAIL = (await page.eval(`document.querySelector('main').innerText`)).match(/invited (\S+@\S+) to/)?.[1];
    assert(EMAIL, "invited address not shown");
    await page.eval(`document.querySelector('form button[type=submit]').click()`);
    await page.waitFor(`document.body.innerText.includes('Check your email')`);
  } else {
    step(`sign-up link for ${EMAIL}`);
    r = await api("/api/auth/sign-in/magic-link", { body: { email: EMAIL, callbackURL: "/account/passkey" } });
    assert(r.status === 200, `magic link request: ${r.status} ${r.text}`);
  }
  let mail = null;
  for (let i = 0; i < 50 && !mail; i++) {
    mail = newestMail(t0 - 1000);
    if (!mail) await sleep(100);
  }
  assert(mail, "no email written under .wrangler/tmp/email");
  const link = mail.match(/https?:\/\/\S+magic-link\/verify\S+/)?.[0];
  assert(link, "no verify link in the email");

  step("open the link: lands on the passkey set-up page");
  await page.goto(link);
  await page.waitFor("location.pathname === '/account/passkey'");
  if (INVITE) {
    assert((await page.eval("location.search")).includes("next=%2Finvite%2F"), "passkey page lost the way back");
  }

  if (!INVITE) {
    step("gate: /account redirects back to passkey set-up until one exists");
    await page.goto(BASE + "/account");
    assert((await page.path()) === "/account/passkey", "gate did not redirect");
  }

  step("register the first passkey");
  await page.eval(`document.querySelector('#add-passkey input[name=name]').value = 'E2E key'`);
  await page.eval(`document.querySelector('#add-passkey button[type=submit]').click()`);
  if (INVITE) {
    step("back on the invitation, signed in: join");
    await page.waitFor("location.pathname.startsWith('/invite/')");
    await page.eval(`document.querySelector('form button[type=submit]').click()`);
    await page.waitFor(`document.body.innerText.includes('You are in')`);
    await page.goto(BASE + "/account");
    const text = await page.eval(`document.querySelector('main').innerText`);
    assert(/you are (a reader|an editor)/.test(text), `project not listed on the account page:\n${text}`);
    console.log(`joined as ${EMAIL}`);
  } else {
    await page.waitFor("location.pathname === '/account'");
    assert(await page.eval(`document.body.innerText.includes('E2E key')`), "passkey not listed");
  }

  if (APPROVE) {
    step(`approve device code ${APPROVE}`);
    await page.goto(`${BASE}/device?user_code=${encodeURIComponent(APPROVE)}`);
    await page.eval(`document.querySelector('button[value=approve]').click()`);
    await page.waitFor(`document.body.innerText.includes('Approved')`);
    await page.send("WebAuthn.removeVirtualAuthenticator", { authenticatorId });
    console.log(`approved as ${EMAIL}`);
    process.exit(0);
  }

  step("email links are now refused for this account");
  r = await api("/api/auth/sign-in/magic-link", { body: { email: EMAIL, callbackURL: "/account" } });
  assert(r.status === 403, `expected 403, got ${r.status} ${r.text}`);

  step("sign out");
  await page.eval(`document.querySelector('form[action="/logout"] button').click()`);
  await page.waitFor("location.pathname === '/'");
  await page.goto(BASE + "/account");
  assert((await page.path()) === "/login", "still signed in after logout");

  step("sign in with the passkey");
  await page.goto(BASE + "/login?callbackURL=%2Faccount");
  await page.eval(`document.getElementById('passkey-signin').click()`);
  await page.waitFor("location.pathname === '/account'");

  step("device flow: app asks for a code");
  r = await api("/api/auth/device/code", { body: { client_id: "wordy-app" } });
  assert(r.status === 200 && r.json?.user_code, `device code: ${r.status} ${r.text}`);
  const { user_code, device_code, interval } = r.json;
  r = await api("/api/auth/device/code", { body: { client_id: "someone-else" } });
  assert(r.status >= 400, "unknown client id was accepted");

  step("device flow: not yet approved");
  const poll = () =>
    api("/api/auth/device/token", {
      body: { grant_type: "urn:ietf:params:oauth:grant-type:device_code", device_code, client_id: "wordy-app" },
    });
  r = await poll();
  assert(r.status === 400 && r.json?.error === "authorization_pending", `pending: ${r.status} ${r.text}`);

  step("device flow: approve in the browser");
  await page.goto(`${BASE}/device?user_code=${encodeURIComponent(user_code)}`);
  assert(await page.eval(`document.querySelector('input[name=user_code]').value === ${JSON.stringify(user_code)}`), "code not prefilled");
  await page.eval(`document.querySelector('button[value=approve]').click()`);
  await page.waitFor(`document.body.innerText.includes('Approved')`);

  step("device flow: app receives a token");
  await sleep((interval ?? 5) * 1000 + 200);
  r = await poll();
  assert(r.status === 200 && r.json?.access_token, `token: ${r.status} ${r.text}`);
  const token = r.json.access_token;

  step("register the device and list it");
  r = await api("/api/devices", { token, body: { name: "E2E box", platform: "linux", appVersion: "0.1.0" } });
  assert(r.status === 200 && r.json?.device?.name === "E2E box", `register: ${r.status} ${r.text}`);
  const deviceId = r.json.device.id;
  r = await api("/api/devices", { method: "GET", token });
  assert(r.status === 200 && r.json.devices.length === 1 && r.json.user.email === EMAIL, `list: ${r.text}`);
  await page.goto(BASE + "/account");
  assert(await page.eval(`document.body.innerText.includes('E2E box')`), "device not shown on account page");

  step("unlink the device from the account page: its token stops working");
  await page.eval(`document.querySelector('input[value=${JSON.stringify(deviceId)}]').form.querySelector('button').click()`);
  await page.waitFor(`document.body.innerText.includes('Device unlinked')`);
  r = await api("/api/devices", { method: "GET", token });
  assert(r.status === 401, `revoked token still works: ${r.status}`);

  step("the only passkey cannot be removed");
  assert(await page.eval(`document.querySelector('form input[value=delete-passkey]').form.querySelector('button').disabled`), "remove button enabled");

  await page.send("WebAuthn.removeVirtualAuthenticator", { authenticatorId });
  console.log(`\nall good for ${EMAIL}`);
} finally {
  child.kill();
}
