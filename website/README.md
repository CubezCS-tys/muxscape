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

Live on Vercel (project `muxscape`). `https://muxscape.com` is canonical (also
`url` in `src/site.config.ts`). `muxscape.dev`, `www.muxscape.dev` and
`www.muxscape.com` are attached to the project with a domain-level 308
redirect to `muxscape.com` (Vercel → Project → Settings → Domains). DNS is at
Cloudflare, records set to "DNS only". `vercel.json` sets the build and
security headers.

Deploy from this folder: `npx vercel deploy --prod`. If the repository is
connected to Vercel later, set the project's **Root Directory** to `website`.

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
