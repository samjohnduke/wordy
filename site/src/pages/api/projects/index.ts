// The projects the signed-in user belongs to (bearer session from the app).
import type { APIRoute } from "astro";
import { listProjects } from "~/server/projects";
import { json } from "~/server/rooms/http";

export const prerender = false;

export const GET: APIRoute = async ({ locals }) => {
  const { user, session } = locals;
  if (!user || !session) return json({ error: "Not signed in." }, 401);
  const projects = await listProjects(user.id);
  return json({
    projects: projects.map(({ project, role }) => ({
      id: project.id,
      name: project.name,
      role,
      owner: project.ownerId === user.id,
      updatedAt: project.updatedAt,
    })),
  });
};
