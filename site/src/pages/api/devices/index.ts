// The app's view of linked devices. Needs a bearer session from the device flow.
import type { APIRoute } from "astro";
import { cleanName, cleanPlatform, listDevices, registerDevice, touchDevice } from "~/server/devices";

export const prerender = false;

function json(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", "cache-control": "no-store" },
  });
}

export const GET: APIRoute = async ({ locals }) => {
  const { user, session } = locals;
  if (!user || !session) return json({ error: "Not signed in." }, 401);
  await touchDevice(session.id);
  return json({
    user: { id: user.id, email: user.email, name: user.name },
    devices: await listDevices(user.id, session.id),
  });
};

export const POST: APIRoute = async ({ locals, request }) => {
  const { user, session } = locals;
  if (!user || !session) return json({ error: "Not signed in." }, 401);
  let body: Record<string, unknown>;
  try {
    body = (await request.json()) as Record<string, unknown>;
  } catch {
    return json({ error: "Expected a JSON body." }, 400);
  }
  const name = cleanName(body.name);
  const platform = cleanPlatform(body.platform);
  if (!name || !platform) return json({ error: "name and platform are required." }, 400);
  const appVersion = typeof body.appVersion === "string" ? body.appVersion.slice(0, 40) : null;
  const device = await registerDevice({ userId: user.id, sessionId: session.id, name, platform, appVersion });
  return json({ user: { id: user.id, email: user.email, name: user.name }, device });
};
