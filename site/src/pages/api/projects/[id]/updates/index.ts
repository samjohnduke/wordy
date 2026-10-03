// PUT a Loro blob too big for the socket. The body is the raw blob; it is
// stored in R2 and appended to the room's log. With `x-wordy-snapshot-at: N`
// the blob is a full snapshot taken after applying sequence N, and becomes the
// room's base when N is still the head.
import { env } from "cloudflare:workers";
import type { APIRoute } from "astro";
import { canWrite } from "~/server/projects";
import { json, roomCaller, updateKey } from "~/server/rooms/http";
import { MAX_BLOB } from "~/server/rooms/protocol";

export const prerender = false;

export const PUT: APIRoute = async (ctx) => {
  const caller = await roomCaller(ctx);
  if (caller instanceof Response) return caller;
  if (!canWrite(caller.role)) return json({ error: "Readers cannot push." }, 403);
  const length = Number(ctx.request.headers.get("content-length") ?? "0");
  if (!(length > 0)) return json({ error: "Expected a blob body." }, 400);
  if (length > MAX_BLOB) return json({ error: "Blob too big." }, 413);
  const snapshotHeader = ctx.request.headers.get("x-wordy-snapshot-at");
  const snapshotAt = snapshotHeader === null ? undefined : Number(snapshotHeader);
  if (snapshotAt !== undefined && !Number.isInteger(snapshotAt)) {
    return json({ error: "x-wordy-snapshot-at must be an integer." }, 400);
  }
  const key = updateKey(caller.projectId, crypto.randomUUID());
  const object = await env.PROJECTS.put(key, ctx.request.body, {
    httpMetadata: { contentType: "application/octet-stream" },
  });
  if (!object) return json({ error: "Could not store the blob." }, 500);
  const result = await caller.room.appendRef({ device: caller.deviceId, key, size: object.size, snapshotAt });
  return json(result);
};
