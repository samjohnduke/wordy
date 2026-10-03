// Owner only: withdraw an invitation.
import type { APIRoute } from "astro";
import { deleteInvitation } from "~/server/projects";
import { json, projectCaller } from "~/server/rooms/http";

export const prerender = false;

export const DELETE: APIRoute = async (ctx) => {
  const caller = await projectCaller(ctx);
  if (caller instanceof Response) return caller;
  if (caller.member.role !== "owner") return json({ error: "Only the owner can withdraw invitations." }, 403);
  if (!(await deleteInvitation(caller.member.project.id, ctx.params.inv ?? ""))) return json({ error: "Not found." }, 404);
  return new Response(null, { status: 204 });
};
