// One place for the launch switches. Leave a link null until it really
// exists: the page then shows an honest "coming soon" state instead.
export const site = {
  name: "Muxscape",
  url: "https://muxscape.com", // primary domain; muxscape.dev redirects here (vercel.json)
  headline: "Your servers. Your terminals. One workspace.",
  tagline: "The SSH workspace that never closes.",
  description:
    "A modern SSH client that turns persistent tmux sessions into clickable tabs and panes. Connect to your servers, manage your work graphically, and pick up exactly where you left off.",
  version: "0.1.0",
  /** Public installer URL. null = not published yet ("coming soon"). */
  downloadUrl: "/download/Muxscape_0.1.0_x64-setup.exe" as string | null,
  /** Shown next to the download: SHA-256 of the installer and its size. */
  downloadSha256: "6636358f5112e2d99ccd16cdade12a5246489bdead81a43a562567db82d650dd" as string | null,
  downloadSize: "5.4 MB" as string | null,
  /** Public repository URL. null = repository still private. */
  githubUrl: "https://github.com/CubezCS-tys/muxscape" as string | null,
};
