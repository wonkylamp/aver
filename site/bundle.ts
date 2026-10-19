import { fileURLToPath } from "node:url";
import type { BuildOptions } from "esbuild";

const here = (path: string) => fileURLToPath(new URL(path, import.meta.url));

/** How the browser bundle is built: build.ts writes viewer.js with it, and the tests bundle src/verify.ts the same way. */
export function browserBundle(entry: string): BuildOptions {
  return {
    entryPoints: [here(entry)],
    bundle: true,
    format: "esm",
    platform: "browser",
    target: ["es2023"],
    legalComments: "eof",
    inject: [here("src/shims/buffer.ts")],
  };
}

export const viewerOutfile = here("viewer.js");
