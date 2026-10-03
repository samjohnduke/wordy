// Session helpers for pages and API routes.
import type { APIContext } from "astro";
import { getAuth } from "./auth";
import { userHasPasskey } from "./users";

export type SessionUser = NonNullable<App.Locals["user"]>;
export type SessionInfo = NonNullable<App.Locals["session"]>;

/** Resolve the session for a request (cookie or bearer) and whether it may do more than add a passkey. */
export async function loadSession(headers: Headers) {
  const result = await getAuth().api.getSession({ headers });
  if (!result) return { user: null, session: null, hasPasskey: false };
  const hasPasskey = await userHasPasskey(result.user.id);
  return { user: result.user, session: result.session, hasPasskey };
}

export function loginUrl(ctx: APIContext): string {
  const back = ctx.url.pathname + ctx.url.search;
  return `/login?callbackURL=${encodeURIComponent(back)}`;
}

/** Pages that a session without a passkey may still use. */
export function allowedWithoutPasskey(pathname: string): boolean {
  return (
    pathname === "/account/passkey" ||
    pathname === "/logout" ||
    pathname.startsWith("/api/auth/") ||
    !pathname.startsWith("/account") && !pathname.startsWith("/device") && !pathname.startsWith("/api/")
  );
}
