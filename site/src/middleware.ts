import { defineMiddleware } from "astro:middleware";
import { allowedWithoutPasskey, loadSession } from "./server/session";

export const onRequest = defineMiddleware(async (context, next) => {
  // Prerendered pages are built without bindings and never see a session.
  if (context.isPrerendered) return next();
  const { user, session, hasPasskey } = await loadSession(context.request.headers);
  context.locals.user = user;
  context.locals.session = session;
  context.locals.hasPasskey = hasPasskey;
  // A fresh account can only add its first passkey until it has one.
  if (session && !hasPasskey && !allowedWithoutPasskey(context.url.pathname)) {
    if (context.url.pathname.startsWith("/api/")) {
      return new Response(JSON.stringify({ error: "Add a passkey to this account first." }), {
        status: 403,
        headers: { "content-type": "application/json" },
      });
    }
    return context.redirect("/account/passkey");
  }
  return next();
});
