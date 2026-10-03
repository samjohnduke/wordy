export const REPO = "samjohnduke/wordy";
export const REPO_URL = `https://github.com/${REPO}`;
export const RELEASES_URL = `${REPO_URL}/releases`;

// Asset names the release workflow uploads. They carry no version so the
// `releases/latest/download/<name>` URLs stay valid across releases.
export const ASSETS = {
  mac: {
    name: "Wordy-macos-arm64.zip",
    label: "macOS",
    detail: "Apple Silicon, macOS 12 or later",
  },
  linux: {
    name: "wordy-linux-x86_64.tar.gz",
    label: "Linux",
    detail: "x86_64, Wayland or X11",
  },
} as const;

export type Platform = keyof typeof ASSETS;
