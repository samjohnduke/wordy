// Who is in a project. Owners also see pending invitations.
import type { APIRoute } from "astro";
import { listInvitations, listMembers } from "~/server/projects";
import { json, projectCaller } from "~/server/rooms/http";

export const prerender = false;

export const GET: APIRoute = async (ctx) => {
  const caller = await projectCaller(ctx);
  if (caller instanceof Response) return caller;
  const { member } = caller;
  const members = await listMembers(member.project.id);
  const invitations = member.role === "owner" ? await listInvitations(member.project.id) : [];
  return json({
    project: { id: member.project.id, name: member.project.name, role: member.role },
    members: members.map((m) => ({ userId: m.userId, email: m.email, role: m.role, since: m.createdAt })),
    invitations: invitations.map((i) => ({ id: i.id, email: i.email, role: i.role, expiresAt: i.expiresAt })),
  });
};
