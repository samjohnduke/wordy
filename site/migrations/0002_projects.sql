-- Projects that have been synced to the cloud, and who may open them.
-- A project row appears the first time a linked device connects its room;
-- that device's user becomes the owner. Roles: owner, editor, reader.
CREATE TABLE IF NOT EXISTS "project" (
  "id" TEXT PRIMARY KEY,
  "ownerId" TEXT NOT NULL REFERENCES "user" ("id") ON DELETE CASCADE,
  "name" TEXT NOT NULL DEFAULT '',
  "createdAt" TEXT NOT NULL,
  "updatedAt" TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS "project_owner" ON "project" ("ownerId");

CREATE TABLE IF NOT EXISTS "membership" (
  "projectId" TEXT NOT NULL REFERENCES "project" ("id") ON DELETE CASCADE,
  "userId" TEXT NOT NULL REFERENCES "user" ("id") ON DELETE CASCADE,
  "role" TEXT NOT NULL,
  "createdAt" TEXT NOT NULL,
  PRIMARY KEY ("projectId", "userId")
);
CREATE INDEX IF NOT EXISTS "membership_user" ON "membership" ("userId");
