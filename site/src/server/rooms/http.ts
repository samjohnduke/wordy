// Shared bits for the HTTP side of project rooms (large blobs and assets).
import { env } from "cloudflare:workers";
import type { APIContext } from "astro";
import { getServerByName } from "partyserver";
import { deviceForSession } from "../devices";
import { ensureMember, membership, validProjectId, type Membership, type Role } from "../projects";
import type { ProjectRoom } from "./project-room";

export function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", "cache-control": "no-store" },
  });
}

export interface RoomCaller {
  projectId: string;
  userId: string;
  deviceId: string;
  role: Role;
  room: DurableObjectStub<ProjectRoom>;
}

/** Bearer session + registered device + membership, or the error response. */
export async function roomCaller(ctx: APIContext): Promise<RoomCaller | Response> {
  const { user, session } = ctx.locals;
  if (!user || !session) return json({ error: "Not signed in." }, 401);
  const projectId = ctx.params.id ?? "";
  if (!validProjectId(projectId)) return json({ error: "Not found." }, 404);
  const device = await deviceForSession(session.id);
  if (!device) return json({ error: "Register this device first." }, 403);
  const member = await ensureMember(projectId, user.id);
  if (!member) return json({ error: "Not your project." }, 403);
  const room = await getServerByName<Env, ProjectRoom>(env.ProjectRoom, projectId);
  return { projectId, userId: user.id, deviceId: device.id, role: member.role, room };
}

/** Any signed-in member of an existing project (no device needed), or the error response. */
export async function projectCaller(ctx: APIContext): Promise<{ user: { id: string; email: string }; member: Membership } | Response> {
  const { user, session } = ctx.locals;
  if (!user || !session) return json({ error: "Not signed in." }, 401);
  const projectId = ctx.params.id ?? "";
  if (!validProjectId(projectId)) return json({ error: "Not found." }, 404);
  const member = await membership(projectId, user.id);
  if (!member) return json({ error: "Not your project." }, 403);
  return { user: { id: user.id, email: user.email }, member };
}

export function updateKey(projectId: string, id: string): string {
  return `projects/${projectId}/updates/${id}.loro`;
}

export function assetKey(projectId: string, sha256: string): string {
  return `projects/${projectId}/assets/${sha256}`;
}
