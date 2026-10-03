// Cloud projects and who may open them. See migrations/0002_projects.sql.
import { env } from "cloudflare:workers";

export type Role = "owner" | "editor" | "reader";

export interface ProjectRow {
  id: string;
  ownerId: string;
  name: string;
  createdAt: string;
  updatedAt: string;
}

export interface Membership {
  project: ProjectRow;
  role: Role;
}

/** Project ids are ULIDs made by the app. */
export function validProjectId(id: string): boolean {
  return /^[0-9A-HJKMNP-TV-Z]{26}$/i.test(id);
}

export function canWrite(role: Role): boolean {
  return role === "owner" || role === "editor";
}

/**
 * The caller's membership of a project. A project nobody has claimed yet is
 * created on the spot with the caller as owner (the app's first connect).
 */
export async function ensureMember(projectId: string, userId: string): Promise<Membership | null> {
  const existing = await membership(projectId, userId);
  if (existing) return existing;
  const project = await env.DB.prepare(`SELECT * FROM project WHERE id = ?`).bind(projectId).first<ProjectRow>();
  if (project) return null; // someone else's
  const now = new Date().toISOString();
  await env.DB.batch([
    env.DB.prepare(`INSERT INTO project (id, ownerId, name, createdAt, updatedAt) VALUES (?, ?, '', ?, ?)`).bind(
      projectId,
      userId,
      now,
      now,
    ),
    env.DB.prepare(`INSERT INTO membership (projectId, userId, role, createdAt) VALUES (?, ?, 'owner', ?)`).bind(
      projectId,
      userId,
      now,
    ),
  ]);
  return membership(projectId, userId);
}

export async function membership(projectId: string, userId: string): Promise<Membership | null> {
  const row = await env.DB.prepare(
    `SELECT p.id, p.ownerId, p.name, p.createdAt, p.updatedAt, m.role
       FROM membership m JOIN project p ON p.id = m.projectId
      WHERE m.projectId = ? AND m.userId = ?`,
  )
    .bind(projectId, userId)
    .first<ProjectRow & { role: Role }>();
  if (!row) return null;
  const { role, ...project } = row;
  return { project, role };
}

/** Every project the user belongs to, newest activity first. */
export async function listProjects(userId: string): Promise<Membership[]> {
  const { results } = await env.DB.prepare(
    `SELECT p.id, p.ownerId, p.name, p.createdAt, p.updatedAt, m.role
       FROM membership m JOIN project p ON p.id = m.projectId
      WHERE m.userId = ? ORDER BY p.updatedAt DESC`,
  )
    .bind(userId)
    .all<ProjectRow & { role: Role }>();
  return results.map(({ role, ...project }) => ({ project, role }));
}

/** Called by the room when a device says hello: keep the name fresh. */
export async function touchProject(projectId: string, name: string | null): Promise<void> {
  const now = new Date().toISOString();
  if (name !== null) {
    await env.DB.prepare(`UPDATE project SET name = ?, updatedAt = ? WHERE id = ?`).bind(name, now, projectId).run();
  } else {
    await env.DB.prepare(`UPDATE project SET updatedAt = ? WHERE id = ?`).bind(now, projectId).run();
  }
}
