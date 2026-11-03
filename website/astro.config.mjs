import { defineConfig } from "astro/config";
import sitemap from "@astrojs/sitemap";
import tailwindcss from "@tailwindcss/vite";
import { site } from "./src/site.config";

export default defineConfig({
  site: site.url,
  integrations: [sitemap()],
  vite: { plugins: [tailwindcss()] },
});
