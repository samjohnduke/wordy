import rss from "@astrojs/rss";
import { getCollection } from "astro:content";
import type { APIContext } from "astro";
import { RELEASES_URL } from "../lib/site";

export async function GET(context: APIContext) {
  const entries = (await getCollection("changelog")).sort(
    (a, b) => b.data.date.getTime() - a.data.date.getTime(),
  );
  return rss({
    title: "Wordy changelog",
    description: "Releases of Wordy, a native writing desk for fiction.",
    site: context.site!,
    items: entries.map((e) => ({
      title: `Wordy ${e.data.version}: ${e.data.title}`,
      pubDate: e.data.date,
      link: `/changelog#v${e.data.version}`,
      description: `${e.data.title}. Downloads: ${RELEASES_URL}/tag/v${e.data.version}`,
      content: e.rendered?.html,
    })),
  });
}
