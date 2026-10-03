// Browser-side better-auth client, only needed for the WebAuthn ceremonies.
import { createAuthClient } from "better-auth/client";
import { passkeyClient } from "@better-auth/passkey/client";

export const authClient = createAuthClient({
  baseURL: window.location.origin,
  plugins: [passkeyClient()],
});

export function errorText(err: { message?: string; code?: string } | null | undefined): string {
  if (!err) return "Something went wrong.";
  return err.message || err.code || "Something went wrong.";
}

/** Wire a "sign in with passkey" button; on success go to `next`. */
export function bindPasskeySignIn(button: HTMLButtonElement, status: HTMLElement, next: string) {
  button.addEventListener("click", async () => {
    button.disabled = true;
    status.textContent = "Waiting for your passkey…";
    const res = await authClient.signIn.passkey();
    if (res?.error) {
      status.textContent = errorText(res.error);
      button.disabled = false;
      return;
    }
    status.textContent = "Signed in.";
    window.location.assign(next);
  });
}

/** Wire an "add passkey" form; on success go to `next`. */
export function bindAddPasskey(form: HTMLFormElement, status: HTMLElement, next: string) {
  form.addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const name = (new FormData(form).get("name") as string | null)?.trim() || undefined;
    const button = form.querySelector<HTMLButtonElement>("button[type=submit]");
    if (button) button.disabled = true;
    status.textContent = "Follow your browser's prompt…";
    const res = await authClient.passkey.addPasskey({ name });
    if (res?.error) {
      status.textContent = errorText(res.error);
      if (button) button.disabled = false;
      return;
    }
    status.textContent = "Passkey added.";
    window.location.assign(next);
  });
}
