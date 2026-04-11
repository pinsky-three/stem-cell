import { defineConfig } from "astro/config";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  outDir: "../public",
  vite: {
    plugins: [tailwindcss()],
  },
});
