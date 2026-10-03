import type { APIRoute } from "astro";
import { revokeDevice } from "~/server/devices";

export const prerender = false;

export const DELETE: APIRoute = async ({ locals, params }) => {
  const { user } = locals;
  if (!user) return new Response(JSON.stringify({ error: "Not signed in." }), { status: 401 });
  const ok = await revokeDevice(user.id, params.id ?? "");
  return new Response(null, { status: ok ? 204 : 404 });
};
