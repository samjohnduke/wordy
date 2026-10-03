// better-auth on D1. Built lazily so that prerendering at build time, which has no
// bindings, never touches it.
import { env } from "cloudflare:workers";
import { betterAuth } from "better-auth";
import { APIError } from "better-auth/api";
import { bearer, deviceAuthorization, magicLink } from "better-auth/plugins";
import { passkey } from "@better-auth/passkey";
import { magicLinkEmail, sendEmail } from "./email";
import { emailHasPasskey } from "./users";

/** The only client id the device flow accepts: the desktop app. */
export const APP_CLIENT_ID = "wordy-app";

/** How long a session lives. Linked devices hold one for as long as they stay in use. */
export const SESSION_DAYS = 365;

function build() {
  const site = new URL(env.SITE_URL);
  const trustedOrigins = new Set([
    site.origin,
    "http://localhost:4321",
    "http://localhost:8787",
  ]);
  return betterAuth({
    appName: "Wordy",
    baseURL: site.origin,
    basePath: "/api/auth",
    secret: env.BETTER_AUTH_SECRET,
    database: env.DB,
    trustedOrigins: [...trustedOrigins],
    session: {
      expiresIn: 60 * 60 * 24 * SESSION_DAYS,
      updateAge: 60 * 60 * 24,
    },
    plugins: [
      magicLink({
        expiresIn: 60 * 15,
        storeToken: "hashed",
        sendMagicLink: async ({ email, url }) => {
          // Email links only create accounts. Once an account has a passkey, that is
          // the only way in.
          if (await emailHasPasskey(email)) {
            throw new APIError("FORBIDDEN", {
              message: "That account signs in with a passkey.",
            });
          }
          await sendEmail(magicLinkEmail({ to: email, url }));
        },
      }),
      passkey({
        rpID: site.hostname,
        rpName: "Wordy",
        origin: site.origin,
        authenticatorSelection: {
          residentKey: "required",
          userVerification: "preferred",
        },
      }),
      bearer(),
      deviceAuthorization({
        expiresIn: "15m",
        interval: "5s",
        verificationUri: `${site.origin}/device`,
        validateClient: (clientId) => clientId === APP_CLIENT_ID,
      }),
    ],
  });
}

let instance: ReturnType<typeof build> | undefined;

export function getAuth() {
  instance ??= build();
  return instance;
}

export type Auth = ReturnType<typeof build>;
