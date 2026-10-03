// Project attachments, stored by content hash. PUT the raw file (the client
// then announces the path over the socket); GET streams it back.
import { env } from "cloudflare:workers";
import type { APIRoute } from "astro";
import { canWrite } from "~/server/projects";
import { assetKey, json, roomCaller } from "~/server/rooms/http";
import { MAX_BLOB } from "~/server/rooms/protocol";

export const prerender = false;

const SHA = /^[0-9a-f]{64}$/;

export const GET: APIRoute = async (ctx) => {
  const caller = await roomCaller(ctx);
  if (caller instanceof Response) return caller;
  const sha = ctx.params.sha ?? "";
  if (!SHA.test(sha)) return json({ error: "Bad hash." }, 400);
  const object = await env.PROJECTS.get(assetKey(caller.projectId, sha));
  if (!object) return json({ error: "No such asset." }, 404);
  return new Response(object.body, {
    headers: {
      "content-type": "application/octet-stream",
      "content-length": String(object.size),
      "cache-control": "private, max-age=31536000, immutable",
    },
  });
};

export const PUT: APIRoute = async (ctx) => {
  const caller = await roomCaller(ctx);
  if (caller instanceof Response) return caller;
  if (!canWrite(caller.role)) return json({ error: "Readers cannot upload." }, 403);
  const sha = ctx.params.sha ?? "";
  if (!SHA.test(sha)) return json({ error: "Bad hash." }, 400);
  const length = Number(ctx.request.headers.get("content-length") ?? "0");
  if (!(length > 0)) return json({ error: "Expected a file body." }, 400);
  if (length > MAX_BLOB) return json({ error: "File too big." }, 413);
  const key = assetKey(caller.projectId, sha);
  if (await env.PROJECTS.head(key)) {
    await ctx.request.body?.cancel();
    return json({ sha256: sha, size: length, existed: true });
  }
  const object = await env.PROJECTS.put(key, ctx.request.body, {
    httpMetadata: { contentType: "application/octet-stream" },
  });
  if (!object) return json({ error: "Could not store the file." }, 500);
  return json({ sha256: sha, size: object.size, existed: false });
};
