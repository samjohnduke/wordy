// Outgoing mail through the Cloudflare `send_email` binding. Under `wrangler dev` the
// binding writes each message to `.wrangler/` as an .eml file instead of sending it.
import { env } from "cloudflare:workers";

export interface Mail {
  to: string;
  subject: string;
  text: string;
  html: string;
}

export async function sendEmail(mail: Mail): Promise<void> {
  await env.EMAIL.send({
    from: env.EMAIL_FROM,
    to: mail.to,
    subject: mail.subject,
    text: mail.text,
    html: mail.html,
  });
}

export function magicLinkEmail({ to, url }: { to: string; url: string }): Mail {
  const subject = "Finish setting up your Wordy account";
  const text = [
    "Open this link to create your Wordy account and add a passkey:",
    "",
    url,
    "",
    "The link works once and expires in 15 minutes.",
    "If you did not ask for this, ignore this email.",
  ].join("\n");
  const html = layout(
    subject,
    `<p>Open the link below to create your Wordy account. The next step adds a passkey,
     which is how you sign in from then on.</p>
     <p><a href="${escapeAttr(url)}" style="${button}">Create my account</a></p>
     <p style="color:#6e685c;font-size:14px">The link works once and expires in 15 minutes.
     If you did not ask for this, ignore this email.</p>`,
  );
  return { to, subject, text, html };
}

const button =
  "display:inline-block;padding:10px 20px;border-radius:999px;background:#9b2f2f;color:#fbf7ef;text-decoration:none;font-weight:600";

function layout(title: string, body: string): string {
  return `<!doctype html><html><body style="margin:0;padding:24px;background:#f6f1e6;color:#1e1d24;font-family:Georgia,serif;font-size:16px;line-height:1.5">
<div style="max-width:36em;margin:0 auto">
<p style="font-weight:700;font-size:20px">Wordy</p>
<h1 style="font-size:22px;margin:0 0 12px">${escapeHtml(title)}</h1>
${body}
</div></body></html>`;
}

function escapeHtml(s: string): string {
  return s.replace(/[&<>"']/g, (c) => `&#${c.charCodeAt(0)};`);
}

function escapeAttr(s: string): string {
  return escapeHtml(s);
}
