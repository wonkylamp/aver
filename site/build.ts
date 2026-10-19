import { fileURLToPath } from "node:url";
import * as esbuild from "esbuild";
import { browserBundle, viewerOutfile } from "./bundle.ts";

await esbuild.build({ ...browserBundle("src/main.ts"), outfile: viewerOutfile, minify: true, logLevel: "info" });

// The landing page needs none of the signing code or its shims.
await esbuild.build({
  entryPoints: [fileURLToPath(new URL("src/landing.ts", import.meta.url))],
  outfile: fileURLToPath(new URL("landing.js", import.meta.url)),
  bundle: true,
  format: "esm",
  platform: "browser",
  target: ["es2023"],
  minify: true,
  logLevel: "info",
});
