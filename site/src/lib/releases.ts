import { ASSETS, REPO, type Platform } from "./site";

export interface ReleaseAsset {
  name: string;
  url: string;
  size: number;
}

export interface Release {
  tag: string;
  version: string;
  url: string;
  publishedAt: Date;
  assets: ReleaseAsset[];
}

interface GithubAsset {
  name: string;
  browser_download_url: string;
  size: number;
}

interface GithubRelease {
  tag_name: string;
  html_url: string;
  published_at: string;
  draft: boolean;
  prerelease: boolean;
  assets: GithubAsset[];
}

/**
 * The newest published GitHub release, fetched once per build. Returns null when
 * there is no release yet or the API is unreachable, so a build never fails on it.
 * Set GITHUB_TOKEN in the build environment to lift the anonymous rate limit.
 */
export async function latestRelease(): Promise<Release | null> {
  const headers: Record<string, string> = {
    Accept: "application/vnd.github+json",
    "User-Agent": "wordy-site",
  };
  const token = process.env.GITHUB_TOKEN;
  if (token) headers.Authorization = `Bearer ${token}`;

  try {
    const res = await fetch(
      `https://api.github.com/repos/${REPO}/releases/latest`,
      { headers },
    );
    if (res.status === 404) return null;
    if (!res.ok) {
      console.warn(
        `[releases] GitHub API ${res.status}; building without download links`,
      );
      return null;
    }
    const data = (await res.json()) as GithubRelease;
    return {
      tag: data.tag_name,
      version: data.tag_name.replace(/^v/, ""),
      url: data.html_url,
      publishedAt: new Date(data.published_at),
      assets: data.assets.map((a) => ({
        name: a.name,
        url: a.browser_download_url,
        size: a.size,
      })),
    };
  } catch (err) {
    console.warn(
      `[releases] ${(err as Error).message}; building without download links`,
    );
    return null;
  }
}

export function assetFor(
  release: Release | null,
  platform: Platform,
): ReleaseAsset | null {
  if (!release) return null;
  return release.assets.find((a) => a.name === ASSETS[platform].name) ?? null;
}

export function formatSize(bytes: number): string {
  if (bytes >= 1 << 20) return `${(bytes / (1 << 20)).toFixed(0)} MB`;
  return `${(bytes / 1024).toFixed(0)} KB`;
}

export function formatDate(d: Date): string {
  return d.toLocaleDateString("en-NZ", {
    year: "numeric",
    month: "long",
    day: "numeric",
    timeZone: "UTC",
  });
}
