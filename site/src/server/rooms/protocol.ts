// The project room's websocket protocol, mirrored in Rust at
// crates/wordy-sync/src/cloud/protocol.rs. Text frames are JSON tagged by `t`.
// Binary frames carry one Loro blob: from the server, prefixed by its 8-byte
// big-endian sequence number; from a client, bare. Blobs bigger than
// MAX_INLINE go over HTTP instead (PUT /api/projects/:id/updates) and are
// announced with a `blob` frame; the client fetches them by sequence number.

export const PROTOCOL_VERSION = 1;
/** Websocket messages on Workers are capped at 1 MiB; keep headroom. */
export const MAX_INLINE = 512 * 1024;
/** Largest blob accepted over HTTP (a whole-novel snapshot is a few MB). */
export const MAX_BLOB = 256 * 1024 * 1024;

export interface AssetEntry {
  path: string;
  sha256: string;
  size: number;
}

export interface Identity {
  userId: string;
  deviceId: string;
  deviceName: string;
  role: "owner" | "editor" | "reader";
}

export type ClientMsg =
  | {
      t: "hello";
      version: number;
      /** Last sequence number this device has applied (0 for never). */
      since: number;
      /** The project's name, to show on the account page. */
      name?: string;
      /** This device's custom dictionary; the room unions it. */
      dictionary?: string[];
    }
  | { t: "dictionary"; words: string[] }
  /** Sent after the file was PUT to /assets/:sha256. */
  | { t: "asset"; path: string; sha256: string; size: number }
  | { t: "ping" };

export type ServerMsg =
  | {
      t: "welcome";
      version: number;
      head: number;
      base: number;
      /** The first sequence number about to be replayed (0 when nothing is). */
      from: number;
      /** The client claimed more than the room has: it must push a snapshot. */
      reset: boolean;
      role: Identity["role"];
      dictionary: string[];
      assets: AssetEntry[];
      /** Size and count of the update log since the base, for compaction. */
      log_bytes: number;
      log_count: number;
    }
  /** A stored update too big for the socket: GET /updates/:seq. */
  | { t: "blob"; seq: number; size: number }
  /** Replay finished; everything up to `head` has been sent. */
  | { t: "synced"; head: number }
  /** Your update was stored (sent to the pushing device only). */
  | { t: "ack"; seq: number }
  | { t: "dictionary"; words: string[] }
  | { t: "asset"; path: string; sha256: string; size: number }
  /** A snapshot became the new base; older updates are gone. */
  | { t: "base_moved"; base: number }
  | { t: "presence"; devices: { device: string; name: string }[] }
  | { t: "pong" }
  | { t: "error"; message: string };

export function frameWithSeq(seq: number, bytes: ArrayBuffer | Uint8Array): Uint8Array {
  const body = bytes instanceof Uint8Array ? bytes : new Uint8Array(bytes);
  const out = new Uint8Array(8 + body.length);
  new DataView(out.buffer).setBigUint64(0, BigInt(seq));
  out.set(body, 8);
  return out;
}
