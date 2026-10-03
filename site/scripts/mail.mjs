// Print the emails that `wrangler dev` wrote instead of sending, newest
// first, with their links pulled out: `pnpm mail` (or `pnpm mail 5`).
// Locally, the send_email binding drops each message as a text file under
// .wrangler/tmp/email/<instance>/email-text/, so this is the inbox.
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join } from "node:path";

const MAIL_DIR = join(process.cwd(), ".wrangler", "tmp", "email");
const limit = Number(process.argv[2] ?? 3);

function collect(dir, out) {
  let names;
  try {
    names = readdirSync(dir);
  } catch {
    return out;
  }
  for (const name of names) {
    const p = join(dir, name);
    const st = statSync(p);
    if (st.isDirectory()) collect(p, out);
    else if (p.includes("email-text") && p.endsWith(".txt")) out.push({ p, mtime: st.mtimeMs });
  }
  return out;
}

const mails = collect(MAIL_DIR, []).sort((a, b) => b.mtime - a.mtime);
if (mails.length === 0) {
  console.log(`No mail under ${MAIL_DIR}. Is wrangler dev running, and did anything send?`);
  process.exit(0);
}
for (const m of mails.slice(0, limit)) {
  const text = readFileSync(m.p, "utf8");
  const links = text.match(/https?:\/\/\S+/g) ?? [];
  console.log(`--- ${new Date(m.mtime).toLocaleString()}  ${m.p.split("/").pop()}`);
  console.log(text.trim());
  if (links.length) console.log(`\nlinks: ${links.join("\n       ")}`);
  console.log();
}
