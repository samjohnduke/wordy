// Direct D1 queries for things better-auth has no endpoint for.
import { env } from "cloudflare:workers";

export async function emailHasPasskey(email: string): Promise<boolean> {
  const row = await env.DB.prepare(
    "SELECT 1 AS one FROM passkey p JOIN user u ON u.id = p.userId WHERE lower(u.email) = lower(?) LIMIT 1",
  )
    .bind(email)
    .first<{ one: number }>();
  return row !== null;
}

export async function userHasPasskey(userId: string): Promise<boolean> {
  const row = await env.DB.prepare("SELECT 1 AS one FROM passkey WHERE userId = ? LIMIT 1")
    .bind(userId)
    .first<{ one: number }>();
  return row !== null;
}

export async function countPasskeys(userId: string): Promise<number> {
  const row = await env.DB.prepare("SELECT count(*) AS n FROM passkey WHERE userId = ?")
    .bind(userId)
    .first<{ n: number }>();
  return row?.n ?? 0;
}
