import { defineCollection } from "astro:content";
import { z } from "astro/zod";
import { glob } from "astro/loaders";

// One Markdown file per release in src/content/changelog/<version>.md. The body is
// the release notes; the release workflow (.github/workflows/release.yml) uses the
// same file as the GitHub Release body when a matching tag is pushed.
const changelog = defineCollection({
  loader: glob({ pattern: "*.md", base: "./src/content/changelog" }),
  schema: z.object({
    version: z.string(),
    date: z.coerce.date(),
    title: z.string(),
  }),
});

export const collections = { changelog };
