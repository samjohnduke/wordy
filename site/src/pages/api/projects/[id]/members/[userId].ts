// Change a member's role (owner) or remove them (owner, or themselves).
import type { APIRoute } from "astro";
import { isGrantableRole } from "~/server/projects";
import { json, projectCaller } from "~/server/rooms/http";
import { changeRole, removeMember, ShareError } from "~/server/sharing";

export const prerender = false;

function failure(err: unknown): Response {
  if (err instanceof ShareError) return json({ error: err.message }, err.status);
  throw err;
}

export const PATCH: APIRoute = async (ctx) => {
  const caller = await projectCaller(ctx);
  if (caller instanceof Response) return caller;
  const body = (await ctx.request.json().catch(() => null)) as { role?: unknown } | null;
  if (!isGrantableRole(body?.role)) return json({ error: "Role must be editor or reader." }, 400);
  try {
    await changeRole(caller.member, ctx.params.userId ?? "", body.role);
  } catch (err) {
    return failure(err);
  }
  return json({ ok: true, role: body.role });
};

export const DELETE: APIRoute = async (ctx) => {
  const caller = await projectCaller(ctx);
  if (caller instanceof Response) return caller;
  try {
    await removeMember(caller.member, ctx.params.userId ?? "", caller.user.id);
  } catch (err) {
    return failure(err);
  }
  return new Response(null, { status: 204 });
};
