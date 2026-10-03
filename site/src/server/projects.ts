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

// ---- sharing ------------------------------------------------------------------

export interface Member {
  userId: string;
  email: string;
  role: Role;
  createdAt: string;
}

export interface Invitation {
  id: string;
  projectId: string;
  email: string;
  role: Role;
  invitedBy: string;
  createdAt: string;
  expiresAt: string;
}

/** Roles an owner can hand out; ownership itself never moves. */
export function isGrantableRole(raw: unknown): raw is Exclude<Role, "owner"> {
  return raw === "editor" || raw === "reader";
}

export const INVITATION_DAYS = 7;

/** Everyone in a project, owner first. */
export async function listMembers(projectId: string): Promise<Member[]> {
  const { results } = await env.DB.prepare(
    `SELECT m.userId, u.email, m.role, m.createdAt
       FROM membership m JOIN user u ON u.id = m.userId
      WHERE m.projectId = ?
      ORDER BY CASE m.role WHEN 'owner' THEN 0 ELSE 1 END, m.createdAt`,
  )
    .bind(projectId)
    .all<Member>();
  return results;
}

/** Invitations that can still be accepted. */
export async function listInvitations(projectId: string): Promise<Invitation[]> {
  const { results } = await env.DB.prepare(
    `SELECT id, projectId, email, role, invitedBy, createdAt, expiresAt FROM invitation
      WHERE projectId = ? AND acceptedAt IS NULL AND expiresAt > ? ORDER BY createdAt`,
  )
    .bind(projectId, new Date().toISOString())
    .all<Invitation>();
  return results;
}

export function cleanEmail(raw: unknown): string | null {
  if (typeof raw !== "string") return null;
  const email = raw.trim().toLowerCase();
  if (email.length > 254 || !/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email)) return null;
  return email;
}

async function sha256Hex(text: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

function newToken(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(24));
  return btoa(String.fromCharCode(...bytes)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

/**
 * Record an invitation and return it with the one-time token for the link.
 * An earlier unaccepted invitation to the same address is replaced.
 */
export async function createInvitation(input: {
  projectId: string;
  email: string;
  role: Exclude<Role, "owner">;
  invitedBy: string;
}): Promise<{ invitation: Invitation; token: string }> {
  const token = newToken();
  const tokenHash = await sha256Hex(token);
  const now = new Date();
  const invitation: Invitation = {
    id: crypto.randomUUID(),
    projectId: input.projectId,
    email: input.email,
    role: input.role,
    invitedBy: input.invitedBy,
    createdAt: now.toISOString(),
    expiresAt: new Date(now.getTime() + INVITATION_DAYS * 86_400_000).toISOString(),
  };
  await env.DB.batch([
    env.DB.prepare(`DELETE FROM invitation WHERE projectId = ? AND lower(email) = ? AND acceptedAt IS NULL`).bind(
      input.projectId,
      input.email,
    ),
    env.DB.prepare(
      `INSERT INTO invitation (id, projectId, email, role, tokenHash, invitedBy, createdAt, expiresAt)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?)`,
    ).bind(
      invitation.id,
      invitation.projectId,
      invitation.email,
      invitation.role,
      tokenHash,
      invitation.invitedBy,
      invitation.createdAt,
      invitation.expiresAt,
    ),
  ]);
  return { invitation, token };
}

export interface InvitationView {
  invitation: Invitation;
  project: ProjectRow;
  inviterEmail: string;
  /** Why it can no longer be accepted, if so. */
  problem: "expired" | "accepted" | null;
}

/** Look an invitation up by the token from its link. */
export async function invitationByToken(token: string): Promise<InvitationView | null> {
  if (!/^[A-Za-z0-9_-]{20,64}$/.test(token)) return null;
  const row = await env.DB.prepare(
    `SELECT i.id, i.projectId, i.email, i.role, i.invitedBy, i.createdAt, i.expiresAt, i.acceptedAt,
            u.email AS inviterEmail
       FROM invitation i JOIN user u ON u.id = i.invitedBy
      WHERE i.tokenHash = ?`,
  )
    .bind(await sha256Hex(token))
    .first<Invitation & { acceptedAt: string | null; inviterEmail: string }>();
  if (!row) return null;
  const project = await env.DB.prepare(`SELECT * FROM project WHERE id = ?`).bind(row.projectId).first<ProjectRow>();
  if (!project) return null;
  const { acceptedAt, inviterEmail, ...invitation } = row;
  const problem = acceptedAt ? "accepted" : invitation.expiresAt <= new Date().toISOString() ? "expired" : null;
  return { invitation, project, inviterEmail, problem };
}

/**
 * Add the user to the project with the invited role and mark the invitation
 * used. An owner stays an owner; anyone else takes the invited role.
 */
export async function acceptInvitation(view: InvitationView, userId: string): Promise<Membership> {
  const now = new Date().toISOString();
  const existing = await membership(view.project.id, userId);
  const statements = [
    env.DB.prepare(`UPDATE invitation SET acceptedAt = ? WHERE id = ? AND acceptedAt IS NULL`).bind(now, view.invitation.id),
  ];
  if (!existing) {
    statements.push(
      env.DB.prepare(`INSERT INTO membership (projectId, userId, role, createdAt) VALUES (?, ?, ?, ?)`).bind(
        view.project.id,
        userId,
        view.invitation.role,
        now,
      ),
    );
  } else if (existing.role !== "owner") {
    statements.push(
      env.DB.prepare(`UPDATE membership SET role = ? WHERE projectId = ? AND userId = ?`).bind(
        view.invitation.role,
        view.project.id,
        userId,
      ),
    );
  }
  await env.DB.batch(statements);
  const result = await membership(view.project.id, userId);
  if (!result) throw new Error("membership missing after accepting");
  return result;
}

export async function deleteInvitation(projectId: string, id: string): Promise<boolean> {
  const r = await env.DB.prepare(`DELETE FROM invitation WHERE id = ? AND projectId = ? AND acceptedAt IS NULL`)
    .bind(id, projectId)
    .run();
  return (r.meta.changes ?? 0) > 0;
}

/** Change a non-owner's role. */
export async function setMemberRole(projectId: string, userId: string, role: Exclude<Role, "owner">): Promise<boolean> {
  const r = await env.DB.prepare(`UPDATE membership SET role = ? WHERE projectId = ? AND userId = ? AND role != 'owner'`)
    .bind(role, projectId, userId)
    .run();
  return (r.meta.changes ?? 0) > 0;
}

/** Remove a non-owner from a project (the owner removing them, or them leaving). */
export async function deleteMembership(projectId: string, userId: string): Promise<boolean> {
  const r = await env.DB.prepare(`DELETE FROM membership WHERE projectId = ? AND userId = ? AND role != 'owner'`)
    .bind(projectId, userId)
    .run();
  return (r.meta.changes ?? 0) > 0;
}
