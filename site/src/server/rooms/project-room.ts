// One Durable Object per project: the ordered log of Loro blobs every device
// pushes, the asset manifest and the dictionary, with live fan-out over
// websockets. The server never runs Loro: a blob is opaque, and clients merge.
//
// Log rows hold the blob inline when it is small, or an R2 key otherwise.
// A client that has synced everything can upload a full snapshot via HTTP
// with `snapshotAt = head`; if nothing else landed meanwhile it becomes the
// new base and older rows are dropped, so fresh devices start from it.
import { Server, type Connection, type ConnectionContext, type WSMessage } from "partyserver";
import { touchProject } from "../projects";
import {
  frameWithSeq,
  MAX_INLINE,
  PROTOCOL_VERSION,
  type AssetEntry,
  type ClientMsg,
  type Identity,
  type ServerMsg,
} from "./protocol";

export const IDENTITY_HEADER = "x-wordy-identity";

type State = Identity & { hello: boolean };

interface UpdateRow {
  seq: number;
  device: string;
  size: number;
  bytes: ArrayBuffer | null;
  r2_key: string | null;
}

export class ProjectRoom extends Server<Env> {
  static options = { hibernate: true };

  onStart() {
    this.sql`CREATE TABLE IF NOT EXISTS updates (
      seq INTEGER PRIMARY KEY, device TEXT NOT NULL, size INTEGER NOT NULL,
      bytes BLOB, r2_key TEXT, created INTEGER NOT NULL)`;
    this.sql`CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)`;
    this.sql`CREATE TABLE IF NOT EXISTS dictionary (word TEXT PRIMARY KEY)`;
    this.sql`CREATE TABLE IF NOT EXISTS assets (
      path TEXT PRIMARY KEY, sha256 TEXT NOT NULL, size INTEGER NOT NULL, updated INTEGER NOT NULL)`;
    this.sql`CREATE TABLE IF NOT EXISTS devices (
      device TEXT PRIMARY KEY, name TEXT NOT NULL, user_id TEXT NOT NULL,
      last_seq INTEGER NOT NULL DEFAULT 0, last_seen INTEGER NOT NULL)`;
  }

  // ---- meta -----------------------------------------------------------------

  private getMeta(key: string, fallback = 0): number {
    const [row] = this.sql<{ value: string }>`SELECT value FROM meta WHERE key = ${key}`;
    return row ? Number(row.value) : fallback;
  }

  private setMeta(key: string, value: number) {
    this.sql`INSERT OR REPLACE INTO meta (key, value) VALUES (${key}, ${String(value)})`;
  }

  get head(): number {
    return this.getMeta("head");
  }

  get base(): number {
    return this.getMeta("base");
  }

  private logStats(): { bytes: number; count: number } {
    const base = this.base;
    const [row] = this.sql<{ bytes: number | null; count: number }>`
      SELECT SUM(size) AS bytes, COUNT(*) AS count FROM updates WHERE seq >= ${base}`;
    return { bytes: row?.bytes ?? 0, count: row?.count ?? 0 };
  }

  // ---- connections ----------------------------------------------------------

  onConnect(conn: Connection<State>, ctx: ConnectionContext) {
    const raw = ctx.request.headers.get(IDENTITY_HEADER);
    if (!raw) return conn.close(4401, "Unauthorised");
    const id = JSON.parse(raw) as Identity;
    conn.setState({ ...id, hello: false });
  }

  onClose(conn: Connection<State>) {
    if (conn.state?.hello) this.sendPresence();
  }

  onError(_conn: Connection<State>, error: unknown) {
    console.error("room", this.name, "socket error", error);
  }

  private send(conn: Connection, msg: ServerMsg) {
    conn.send(JSON.stringify(msg));
  }

  private fail(conn: Connection, message: string, code = 4400) {
    this.send(conn, { t: "error", message });
    conn.close(code, message.slice(0, 120));
  }

  private broadcastMsg(msg: ServerMsg, without?: string[]) {
    this.broadcast(JSON.stringify(msg), without);
  }

  private sendPresence() {
    const devices = [];
    for (const c of this.getConnections<State>()) {
      if (c.state?.hello) devices.push({ device: c.state.deviceId, name: c.state.deviceName });
    }
    this.broadcastMsg({ t: "presence", devices });
  }

  async onMessage(conn: Connection<State>, message: WSMessage) {
    const state = conn.state;
    if (!state) return this.fail(conn, "no identity", 4401);
    if (typeof message === "string") {
      let msg: ClientMsg;
      try {
        msg = JSON.parse(message);
      } catch {
        return this.fail(conn, "bad JSON");
      }
      if (msg.t === "hello") return this.onHello(conn, state, msg);
      if (!state.hello) return this.fail(conn, "say hello first");
      switch (msg.t) {
        case "ping":
          return this.send(conn, { t: "pong" });
        case "dictionary":
          if (state.role === "reader") return;
          return this.addWords(msg.words, conn.id);
        case "asset":
          if (state.role === "reader") return;
          return this.recordAsset(msg, conn.id);
        default:
          return this.fail(conn, `unknown message ${(msg as { t: string }).t}`);
      }
    }
    // Binary: one Loro blob from this device.
    if (!state.hello) return this.fail(conn, "say hello first");
    if (state.role === "reader") return this.fail(conn, "readers cannot push", 4403);
    const bytes = toArrayBuffer(message);
    if (bytes.byteLength === 0) return;
    if (bytes.byteLength > MAX_INLINE) return this.fail(conn, "update too big for the socket; use HTTP");
    const seq = this.store({ device: state.deviceId, bytes, size: bytes.byteLength });
    this.send(conn, { t: "ack", seq });
    this.broadcast(frameWithSeq(seq, bytes));
    this.markSeen(state, seq);
  }

  private async onHello(conn: Connection<State>, state: State, msg: Extract<ClientMsg, { t: "hello" }>) {
    if (msg.version !== PROTOCOL_VERSION) {
      return this.fail(conn, `this server speaks sync protocol v${PROTOCOL_VERSION}, the app v${msg.version}`, 4426);
    }
    conn.setState({ ...state, hello: true });
    const now = Date.now();
    this.sql`INSERT INTO devices (device, name, user_id, last_seq, last_seen)
      VALUES (${state.deviceId}, ${state.deviceName}, ${state.userId}, ${Math.max(0, msg.since | 0)}, ${now})
      ON CONFLICT(device) DO UPDATE SET name = excluded.name, last_seen = excluded.last_seen`;
    if (state.role !== "reader" && msg.dictionary?.length) this.addWords(msg.dictionary, conn.id);

    const head = this.head;
    const base = this.base;
    const since = Math.max(0, msg.since | 0);
    const reset = since > head;
    // Behind the base: replay from the base snapshot (inclusive).
    let from = since + 1;
    if (base > 0 && since < base) from = base;
    if (from > head) from = 0;
    const stats = this.logStats();
    this.send(conn, {
      t: "welcome",
      version: PROTOCOL_VERSION,
      head,
      base,
      from,
      reset,
      role: state.role,
      dictionary: this.words(),
      assets: this.assets(),
      log_bytes: stats.bytes,
      log_count: stats.count,
    });
    if (from > 0) {
      const rows = this.sql<UpdateRow>`SELECT seq, device, size, bytes, r2_key FROM updates
        WHERE seq >= ${from} AND seq <= ${head} ORDER BY seq`;
      for (const row of rows) {
        if (row.bytes) conn.send(frameWithSeq(row.seq, row.bytes));
        else this.send(conn, { t: "blob", seq: row.seq, size: row.size });
      }
    }
    this.send(conn, { t: "synced", head });
    this.sendPresence();
    const name = typeof msg.name === "string" ? msg.name.trim().slice(0, 200) : null;
    this.ctx.waitUntil(touchProject(this.name, name).catch((e) => console.error("touch project", this.name, e)));
  }

  private markSeen(state: State, seq: number) {
    this.sql`UPDATE devices SET last_seq = MAX(last_seq, ${seq}), last_seen = ${Date.now()} WHERE device = ${state.deviceId}`;
  }

  // ---- the log --------------------------------------------------------------

  private store(u: { device: string; size: number; bytes?: ArrayBuffer; r2Key?: string }): number {
    const seq = this.head + 1;
    this.sql`INSERT INTO updates (seq, device, size, bytes, r2_key, created)
      VALUES (${seq}, ${u.device}, ${u.size}, ${u.bytes ? (u.bytes as unknown as string) : null}, ${u.r2Key ?? null}, ${Date.now()})`;
    this.setMeta("head", seq);
    return seq;
  }

  /**
   * RPC from the Worker: a blob that arrived over HTTP and now sits in R2.
   * `snapshotAt` marks it as a full snapshot taken after applying `head`
   * number `snapshotAt`; it becomes the base when nothing landed meanwhile.
   */
  async appendRef(opts: { device: string; key: string; size: number; snapshotAt?: number }): Promise<{ seq: number; base: number }> {
    const headBefore = this.head;
    const seq = this.store({ device: opts.device, size: opts.size, r2Key: opts.key });
    let base = this.base;
    if (opts.snapshotAt !== undefined && opts.snapshotAt === headBefore) {
      base = await this.moveBase(seq);
    }
    // The uploader hears the ack first so it can skip the blob notice that follows.
    for (const c of this.getConnections<State>()) {
      if (c.state?.deviceId === opts.device) this.send(c, { t: "ack", seq });
    }
    this.broadcastMsg({ t: "blob", seq, size: opts.size });
    this.sql`UPDATE devices SET last_seq = MAX(last_seq, ${seq}), last_seen = ${Date.now()} WHERE device = ${opts.device}`;
    return { seq, base };
  }

  /** Drop everything before the new base, in SQLite and in R2. */
  private async moveBase(seq: number): Promise<number> {
    const old = this.sql<{ r2_key: string | null }>`SELECT r2_key FROM updates WHERE seq < ${seq} AND r2_key IS NOT NULL`;
    this.sql`DELETE FROM updates WHERE seq < ${seq}`;
    this.setMeta("base", seq);
    const keys = old.map((r) => r.r2_key).filter((k): k is string => !!k);
    if (keys.length) await this.env.PROJECTS.delete(keys).catch((e) => console.error("delete old blobs", e));
    this.broadcastMsg({ t: "base_moved", base: seq });
    return seq;
  }

  /** RPC: where a stored update lives. */
  async getUpdate(seq: number): Promise<{ bytes: ArrayBuffer } | { key: string } | null> {
    const [row] = this.sql<UpdateRow>`SELECT seq, device, size, bytes, r2_key FROM updates WHERE seq = ${seq}`;
    if (!row) return null;
    if (row.bytes) return { bytes: row.bytes };
    if (row.r2_key) return { key: row.r2_key };
    return null;
  }

  /** RPC: numbers for the account page. */
  async summary() {
    const stats = this.logStats();
    const devices = this.sql<{ device: string; name: string; last_seq: number; last_seen: number }>`
      SELECT device, name, last_seq, last_seen FROM devices ORDER BY last_seen DESC`;
    return { head: this.head, base: this.base, ...stats, devices, assets: this.assets().length };
  }

  // ---- dictionary and assets ------------------------------------------------

  private words(): string[] {
    return this.sql<{ word: string }>`SELECT word FROM dictionary ORDER BY word`.map((r) => r.word);
  }

  private addWords(words: string[], fromConn: string) {
    const added: string[] = [];
    for (const raw of words) {
      if (typeof raw !== "string") continue;
      const word = raw.trim();
      if (!word || word.length > 100) continue;
      const [exists] = this.sql`SELECT 1 AS x FROM dictionary WHERE word = ${word}`;
      if (exists) continue;
      this.sql`INSERT INTO dictionary (word) VALUES (${word})`;
      added.push(word);
    }
    if (added.length) this.broadcastMsg({ t: "dictionary", words: added }, [fromConn]);
  }

  private assets(): AssetEntry[] {
    return this.sql<{ path: string; sha256: string; size: number }>`SELECT path, sha256, size FROM assets ORDER BY path`;
  }

  private recordAsset(a: AssetEntry, fromConn: string) {
    if (!safeRelative(a.path) || !/^[0-9a-f]{64}$/.test(a.sha256) || !(a.size >= 0)) return;
    this.sql`INSERT INTO assets (path, sha256, size, updated) VALUES (${a.path}, ${a.sha256}, ${a.size | 0}, ${Date.now()})
      ON CONFLICT(path) DO UPDATE SET sha256 = excluded.sha256, size = excluded.size, updated = excluded.updated`;
    this.broadcastMsg({ t: "asset", path: a.path, sha256: a.sha256, size: a.size | 0 }, [fromConn]);
  }
}

/** Relative, forward slashes, no parent or hidden components. */
export function safeRelative(path: string): boolean {
  if (typeof path !== "string" || !path || path.length > 1024 || path.includes("\\") || path.includes("\0")) return false;
  return path.split("/").every((c) => c !== "" && c !== "." && c !== "..");
}

function toArrayBuffer(message: WSMessage): ArrayBuffer {
  if (message instanceof ArrayBuffer) return message;
  const view = message as ArrayBufferView;
  return view.buffer.slice(view.byteOffset, view.byteOffset + view.byteLength) as ArrayBuffer;
}
