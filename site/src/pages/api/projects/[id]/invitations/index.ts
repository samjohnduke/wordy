// Owner only: invite someone by email.
import type { APIRoute } from "astro";
import { cleanEmail, isGrantableRole } from "~/server/projects";
import { json, projectCaller } from "~/server/rooms/http";
import { inviteByEmail, ShareError } from "~/server/sharing";

export const prerender = false;

export const POST: APIRoute = async (ctx) => {
  const caller = await projectCaller(ctx);
  if (caller instanceof Response) return caller;
  const body = (await ctx.request.json().catch(() => null)) as { email?: unknown; role?: unknown } | null;
  const email = cleanEmail(body?.email);
  if (!email) return json({ error: "Enter an email address." }, 400);
  if (!isGrantableRole(body?.role)) return json({ error: "Role must be editor or reader." }, 400);
  try {
    const invitation = await inviteByEmail({ project: caller.member, by: caller.user, email, role: body.role });
    return json({ invitation: { id: invitation.id, email: invitation.email, role: invitation.role, expiresAt: invitation.expiresAt } });
  } catch (err) {
    if (err instanceof ShareError) return json({ error: err.message }, err.status);
    throw err;
  }
};
