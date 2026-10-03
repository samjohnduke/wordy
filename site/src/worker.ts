// Worker entry: Astro handles pages and API routes; project rooms (Durable
// Objects) take the websocket upgrades under /parties/project-room/:id.
import { handle } from "@astrojs/cloudflare/handler";
import { routePartykitRequest } from "partyserver";
import { getAuth } from "./server/auth";
import { deviceForSession } from "./server/devices";
import { ensureMember, validProjectId } from "./server/projects";
import { IDENTITY_HEADER } from "./server/rooms/project-room";
import type { Identity } from "./server/rooms/protocol";

export { ProjectRoom } from "./server/rooms/project-room";

export default {
  async fetch(request, env, ctx) {
    const { pathname } = new URL(request.url);
    if (pathname.startsWith("/parties/")) {
      const res = await routePartykitRequest(request, env, {
        onBeforeConnect: async (req, lobby) => {
          if (lobby.className !== "ProjectRoom" || !validProjectId(lobby.name)) {
            return new Response("Not found", { status: 404 });
          }
          const session = await getAuth().api.getSession({ headers: req.headers });
          if (!session) return new Response("Not signed in", { status: 401 });
          const device = await deviceForSession(session.session.id);
          if (!device) return new Response("Register this device first", { status: 403 });
          const member = await ensureMember(lobby.name, session.user.id);
          if (!member) return new Response("Not your project", { status: 403 });
          const identity: Identity = {
            userId: session.user.id,
            deviceId: device.id,
            deviceName: device.name,
            role: member.role,
          };
          const forwarded = new Request(req);
          forwarded.headers.set(IDENTITY_HEADER, JSON.stringify(identity));
          return forwarded;
        },
        onBeforeRequest: () => new Response("Not found", { status: 404 }),
      });
      if (res) return res;
    }
    return handle(request, env, ctx);
  },
} satisfies ExportedHandler<Env>;
