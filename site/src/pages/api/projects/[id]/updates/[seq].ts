// GET one stored update by sequence number (announced over the socket as
// `blob` when it was too big to send inline).
import { env } from "cloudflare:workers";
import type { APIRoute } from "astro";
import { json, roomCaller } from "~/server/rooms/http";

export const prerender = false;

export const GET: APIRoute = async (ctx) => {
  const caller = await roomCaller(ctx);
  if (caller instanceof Response) return caller;
  const seq = Number(ctx.params.seq);
  if (!Number.isInteger(seq) || seq <= 0) return json({ error: "Bad sequence number." }, 400);
  const found = await caller.room.getUpdate(seq);
  if (!found) return json({ error: "No such update (compacted away?)." }, 404);
  const headers = { "content-type": "application/octet-stream", "cache-control": "private, max-age=3600" };
  if ("bytes" in found) return new Response(found.bytes, { headers });
  const object = await env.PROJECTS.get(found.key);
  if (!object) return json({ error: "Blob missing from storage." }, 404);
  return new Response(object.body, { headers });
};
