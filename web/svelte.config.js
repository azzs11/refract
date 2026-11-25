import adapter from "@sveltejs/adapter-static";
import { vitePreprocess } from "@sveltejs/vite-plugin-svelte";

/** @type {import('@sveltejs/kit').Config} */
export default {
  preprocess: vitePreprocess(),
  kit: {
    // Static files in dist/ (deployed to GitHub Pages by .github/workflows/release.yml).
    adapter: adapter({ pages: "dist", assets: "dist", strict: true }),
    // Relative asset paths: the site works under any base path (e.g. /refract/ on Pages).
    paths: { relative: true },
  },
};
