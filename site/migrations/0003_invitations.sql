-- Invitations to share a project. The emailed link carries a random token;
-- only its hash is stored. Accepting creates a membership row.
CREATE TABLE IF NOT EXISTS "invitation" (
  "id" TEXT PRIMARY KEY,
  "projectId" TEXT NOT NULL REFERENCES "project" ("id") ON DELETE CASCADE,
  "email" TEXT NOT NULL,
  "role" TEXT NOT NULL,
  "tokenHash" TEXT NOT NULL UNIQUE,
  "invitedBy" TEXT NOT NULL REFERENCES "user" ("id") ON DELETE CASCADE,
  "createdAt" TEXT NOT NULL,
  "expiresAt" TEXT NOT NULL,
  "acceptedAt" TEXT
);
CREATE INDEX IF NOT EXISTS "invitation_project" ON "invitation" ("projectId");
