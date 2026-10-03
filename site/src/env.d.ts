declare namespace Cloudflare {
  interface Env {
    BETTER_AUTH_SECRET: string;
  }
}

declare namespace App {
  interface Locals {
    user: import("better-auth").User | null;
    session: import("better-auth").Session | null;
    /** False for a brand-new account that has not registered a passkey yet. */
    hasPasskey: boolean;
  }
}
