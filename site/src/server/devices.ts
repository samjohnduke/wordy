// Linked copies of the app. Each one is a bearer session issued by the device flow,
// plus a row here with the machine's name so the account page can show and revoke it.
import { env } from "cloudflare:workers";

export interface Device {
  id: string;
  name: string;
  platform: string;
  appVersion: string | null;
  createdAt: string;
  lastSeenAt: string;
  /** Whether this row belongs to the session making the request. */
  current: boolean;
}

interface Row {
  id: string;
  sessionId: string;
  name: string;
  platform: string;
  appVersion: string | null;
  createdAt: string;
  lastSeenAt: string;
}

const NAME_MAX = 80;

export function cleanName(raw: unknown): string | null {
  if (typeof raw !== "string") return null;
  const name = raw.trim().replace(/\s+/g, " ");
  if (!name || name.length > NAME_MAX) return null;
  return name;
}

export function cleanPlatform(raw: unknown): string | null {
  if (typeof raw !== "string") return null;
  const p = raw.trim().toLowerCase();
  return /^[a-z0-9_-]{1,32}$/.test(p) ? p : null;
}

/** Create or refresh the device row for a session. */
export async function registerDevice(input: {
  userId: string;
  sessionId: string;
  name: string;
  platform: string;
  appVersion: string | null;
}): Promise<Device> {
  const now = new Date().toISOString();
  const id = crypto.randomUUID();
  await env.DB.prepare(
    `INSERT INTO device (id, userId, sessionId, name, platform, appVersion, createdAt, lastSeenAt)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?)
     ON CONFLICT(sessionId) DO UPDATE SET
       name = excluded.name, platform = excluded.platform,
       appVersion = excluded.appVersion, lastSeenAt = excluded.lastSeenAt`,
  )
    .bind(id, input.userId, input.sessionId, input.name, input.platform, input.appVersion, now, now)
    .run();
  const row = await env.DB.prepare("SELECT * FROM device WHERE sessionId = ?")
    .bind(input.sessionId)
    .first<Row>();
  if (!row) throw new Error("device row missing after upsert");
  return toDevice(row, input.sessionId);
}

export async function touchDevice(sessionId: string): Promise<void> {
  await env.DB.prepare("UPDATE device SET lastSeenAt = ? WHERE sessionId = ?")
    .bind(new Date().toISOString(), sessionId)
    .run();
}

export async function listDevices(userId: string, currentSessionId: string): Promise<Device[]> {
  const { results } = await env.DB.prepare(
    "SELECT * FROM device WHERE userId = ? ORDER BY lastSeenAt DESC",
  )
    .bind(userId)
    .all<Row>();
  return results.map((r) => toDevice(r, currentSessionId));
}

/**
 * Remove a device and the session behind it, so its token stops working at once.
 * Returns false when the device is not this user's.
 */
export async function revokeDevice(userId: string, deviceId: string): Promise<boolean> {
  const row = await env.DB.prepare("SELECT sessionId FROM device WHERE id = ? AND userId = ?")
    .bind(deviceId, userId)
    .first<{ sessionId: string }>();
  if (!row) return false;
  await env.DB.batch([
    env.DB.prepare("DELETE FROM device WHERE id = ?").bind(deviceId),
    env.DB.prepare("DELETE FROM session WHERE id = ?").bind(row.sessionId),
  ]);
  return true;
}

function toDevice(r: Row, currentSessionId: string): Device {
  return {
    id: r.id,
    name: r.name,
    platform: r.platform,
    appVersion: r.appVersion,
    createdAt: r.createdAt,
    lastSeenAt: r.lastSeenAt,
    current: r.sessionId === currentSessionId,
  };
}
