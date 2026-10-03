// @ts-check
import { defineConfig } from "astro/config";
import cloudflare from "@astrojs/cloudflare";

// Public site for Wordy: landing page, downloads, changelog. Every page is
// prerendered at build time; the Cloudflare adapter serves them from the
// Worker's static assets and gives us on-demand rendering if a page ever needs it.
export default defineConfig({
  // Canonical URL used for the RSS feed and Open Graph tags. Change it when the
  // Worker gets a custom domain (see the commented-out "routes" in wrangler.jsonc).
  site: "https://wordy.samduke.dev",
  adapter: cloudflare(),
  trailingSlash: "never",
  build: { format: "file" },
});
