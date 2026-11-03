// One place for the launch switches. Leave a link null until it really
// exists: the page then shows an honest "coming soon" state instead.
export const site = {
  name: "Muxscape",
  url: "https://muxscape.dev", // placeholder domain: change before deploying
  headline: "Your servers. Your terminals. One workspace.",
  tagline: "The SSH workspace that never closes.",
  description:
    "A modern SSH client that turns persistent tmux sessions into clickable tabs and panes. Connect to your servers, manage your work graphically, and pick up exactly where you left off.",
  version: "0.1.0",
  /** Public installer URL (e.g. a GitHub release asset). null = not published yet. */
  downloadUrl: null as string | null,
  /** Public repository URL. null = repository still private. */
  githubUrl: null as string | null,
};
