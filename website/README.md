# Muxscape website

Static marketing site (Astro + TypeScript + Tailwind CSS 4). Separate from the
desktop app: nothing here affects the Tauri build.

```sh
npm install
npm run dev       # http://localhost:4321
npm run build     # type-checks, then outputs static files to dist/
npm run preview
```

## Deploying (Vercel)

`vercel.json` sets the build and redirects `muxscape.dev`, `www.muxscape.dev`
and `www.muxscape.com` to `https://muxscape.com` (the canonical domain, also
set as `url` in `src/site.config.ts`). In Vercel, set the project's **Root
Directory** to `website`, add all four domains to the project, and point DNS
at Vercel (apex: `A 76.76.21.21`; `www`: `CNAME cname.vercel-dns.com`), or
use Vercel's nameservers.

## Before launch

Edit `src/site.config.ts`:

- `url`: the canonical domain (`https://muxscape.com`), used for canonical URLs, the sitemap and social cards.
- `downloadUrl`: the public installer (e.g. the GitHub release asset). While it's
  `null`, download buttons show "Windows alpha: coming soon".
- `githubUrl`: the public repository. While it's `null`, GitHub links are inert.

## Screenshots

`src/assets/screens/*.png` are real screenshots of the `main` build, taken
with the browser dev bridge against the Docker test servers (`dev/test-server`):
a small Node project with real output in a 22-window tmux session. The host
names (dev-box, staging, nas) are labels for those test servers. `public/og.png`
is the social preview (1200×630) built from the hero screenshot.

### Still to record

The "See it work" section has two clearly marked placeholders:

1. **Resume after a restart.** Start a long build, quit Muxscape (or drop
   Wi-Fi), reopen, and show the workspace and build still there. A 10–20 s
   clip or GIF.
2. **Long-running coding agents.** A real agent session (Claude Code, Codex…)
   working in a pane, then the same session after reconnecting later.

Replace each placeholder `<div>` in `src/pages/index.astro` with an `<Image>`
or a `<video>` once recorded.
