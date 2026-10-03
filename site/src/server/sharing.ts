// Sharing a project: invitations by email, roles, and telling the project's
// room when someone's access changes so open connections follow suit.
import { env } from "cloudflare:workers";
import { getServerByName } from "partyserver";
import { invitationEmail, sendEmail } from "./email";
import {
  acceptInvitation,
  createInvitation,
  deleteInvitation,
  deleteMembership,
  invitationByToken,
  listMembers,
  membership,
  setMemberRole,
  type Invitation,
  type InvitationView,
  type Membership,
  type Role,
} from "./projects";
import type { ProjectRoom } from "./rooms/project-room";

export class ShareError extends Error {
  constructor(
    message: string,
    public status = 400,
  ) {
    super(message);
  }
}

export function invitationUrl(token: string): string {
  return new URL(`/invite/${token}`, env.SITE_URL).toString();
}

/** Owner only: email an invitation. Returns it (the token is only in the email). */
export async function inviteByEmail(input: {
  project: Membership;
  by: { id: string; email: string };
  email: string;
  role: Exclude<Role, "owner">;
}): Promise<Invitation> {
  if (input.project.role !== "owner") throw new ShareError("Only the owner can invite people.", 403);
  if (input.email === input.by.email.toLowerCase()) throw new ShareError("That is your own address.");
  const members = await listMembers(input.project.project.id);
  if (members.some((m) => m.email.toLowerCase() === input.email)) {
    throw new ShareError("That person is already in the project.");
  }
  const { invitation, token } = await createInvitation({
    projectId: input.project.project.id,
    email: input.email,
    role: input.role,
    invitedBy: input.by.id,
  });
  await sendEmail(
    invitationEmail({
      to: input.email,
      url: invitationUrl(token),
      projectName: input.project.project.name,
      inviterEmail: input.by.email,
      role: input.role,
    }),
  );
  return invitation;
}

/** The signed-in user takes up an invitation addressed to their email. */
export async function acceptByToken(token: string, user: { id: string; email: string }): Promise<Membership> {
  const view = await invitationByToken(token);
  if (!view) throw new ShareError("This invitation does not exist.", 404);
  if (view.problem === "expired") throw new ShareError("This invitation has expired.", 410);
  if (view.problem === "accepted") throw new ShareError("This invitation was already used.", 410);
  if (!invitationIsFor(view, user.email)) throw new ShareError("This invitation was sent to a different address.", 403);
  const result = await acceptInvitation(view, user.id);
  await roomMemberChanged(view.project.id, user.id, result.role);
  return result;
}

export function invitationIsFor(view: InvitationView, email: string): boolean {
  return view.invitation.email.toLowerCase() === email.toLowerCase();
}

/** Owner only: give a member a new role. */
export async function changeRole(project: Membership, userId: string, role: Exclude<Role, "owner">): Promise<void> {
  if (project.role !== "owner") throw new ShareError("Only the owner can change roles.", 403);
  if (!(await setMemberRole(project.project.id, userId, role))) throw new ShareError("No such member.", 404);
  await roomMemberChanged(project.project.id, userId, role);
}

/** The owner removing a member, or a member leaving. */
export async function removeMember(project: Membership, userId: string, byUserId: string): Promise<void> {
  if (project.role !== "owner" && userId !== byUserId) throw new ShareError("Only the owner can remove people.", 403);
  if (userId === project.project.ownerId) throw new ShareError("The owner cannot leave their own project.");
  if (!(await deleteMembership(project.project.id, userId))) throw new ShareError("No such member.", 404);
  await roomMemberChanged(project.project.id, userId, null);
}

/** Owner only: withdraw an invitation. */
export async function deleteInvitationFor(project: Membership, id: string): Promise<boolean> {
  if (project.role !== "owner") throw new ShareError("Only the owner can withdraw invitations.", 403);
  return deleteInvitation(project.project.id, id);
}

/** Tell the room: open connections of this user get the new role, or are closed. */
async function roomMemberChanged(projectId: string, userId: string, role: Role | null): Promise<void> {
  try {
    const room = await getServerByName<Env, ProjectRoom>(env.ProjectRoom, projectId);
    await room.memberChanged(userId, role);
  } catch (e) {
    console.error("room member change", projectId, e);
  }
}

export { membership };
