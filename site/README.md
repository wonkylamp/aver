# Aver website

A static site with no build step at deploy time:

- `index.html` is the landing page. It loads `landing.js`, which runs the Copy buttons and the cause explorer.
- `receipt.html` is the receipt viewer. It loads `viewer.js`.
- `404.html` is the not-found page. It uses root-relative URLs because Cloudflare Pages serves it at any unknown path.
- `img/` holds the agents-use-Aver diagram, light and dark, copied from `docs/img/`.
- `style.css` styles all three pages. `fonts/` holds the two self-hosted typefaces, Atkinson Hyperlegible Next and JetBrains Mono (latin subsets, variable weight), with their SIL Open Font License texts. The pages load nothing from other servers.

Both scripts are committed. `build.ts` builds them with esbuild:

- `viewer.js` from `src/main.ts`. It bundles the canonical JSON, id and signing code in `src/core/`, plus `@noble/ed25519` and `@noble/hashes`. `src/shims/buffer.ts` stands in for Node's `Buffer`, which that code uses.
- `landing.js` from `src/landing.ts`. The cause explorer runs `src/rules.ts`, a port of `crates/aver-core/src/rules.rs`. `test/rules.test.ts` repeats the Rust module's test tables against it, and `test/explorer.test.ts` checks that the rule list in `index.html` matches it.

## Commands

| Command | What it does |
|---|---|
| `make site` | Installs the `site/` dependencies if missing and rebuilds `viewer.js` and `landing.js`. Commit the result. |
| `cd site && npm test` | Rules, explorer, verification, rendering, bundle and landing-page content tests (`node:test`). `test/landing.test.ts` checks the install commands against README.md and docs/, the Git to Aver table's commands against `crates/aver/src/cli`, the section links, the locked positioning and the `{{NUM:…}}` benchmark placeholders. `test/drafts.test.ts` checks the launch drafts in `../launch-drafts/` (lengths, sections, no dollar amounts). |
| `cd site && npm run typecheck` | `tsc --noEmit` over the site sources. |
| `python3 -m http.server -d site 8000` | Serves the site at `http://127.0.0.1:8000`. Open it over HTTP: browsers block ES modules on `file://`. This server does not apply `_headers` or serve `404.html` for unknown paths. |

## Viewer URL parameters

- `?url=<receipt URL>` fetches a receipt. A relative URL resolves against the page. A URL on another origin works only if that server sends CORS headers.
- `?key=<verifier public key>` fills the key field: the `public_key` of `$AVER_HOME/keys/server.json`, or the action's `public-key.txt`, 43 characters of unpadded base64url.

## Deploy

The site is the `site/` directory as it is in Git, served at `https://aver.build`. The sources, tests and build files in it are not needed at runtime and are harmless to serve. `site/node_modules/` is ignored by Git and must not be uploaded; `wrangler pages deploy` skips `node_modules` directories.

### Cloudflare Pages

`_headers` sets the response headers for every path: a Content-Security-Policy that allows only the site's own scripts, styles, fonts and images (plus `https:` fetches for the viewer's `?url=`), `frame-ancestors 'none'`, HSTS, `X-Content-Type-Options`, `Referrer-Policy` and `Permissions-Policy`. Each page's meta CSP narrows the policy further. Cloudflare joins a header that two rules set with a comma, which browsers read as two policies that must both pass, so keep a single CSP rule.

To upload from a clean checkout, after `make site`:

```bash
npx wrangler pages deploy site --project-name aver
```

### The deploy workflow

`.github/workflows/site.yml` runs on pushes to `main` and pull requests that touch `site/**` or `schemas/vectors/**` (the viewer's tests read the vectors), and on manual runs. It installs the `site/` dependencies, runs `npm test`, `npm run typecheck` and `npm run build`, and warns if the build changed the committed `viewer.js` or `landing.js`. Then, on `main` only, it runs `wrangler pages deploy site --project-name=aver --branch=main` through `cloudflare/wrangler-action`, which makes a production deployment of the Pages project `aver`. The workflow has only `contents: read`.

The deploy step runs only when the repository variable `SITE_DEPLOY` is `true`. Until then the workflow builds and tests and deploys nothing. Pull requests never deploy and get no preview URLs. To turn deploys on:

1. Create a Cloudflare API token with the **Cloudflare Pages: Edit** permission on the account.
2. In the repository, open **Settings > Secrets and variables > Actions**. Add the secrets `CLOUDFLARE_API_TOKEN` (the token) and `CLOUDFLARE_ACCOUNT_ID` (the account id from the dashboard). On the **Variables** tab, add `SITE_DEPLOY` with the value `true`.
3. Create the Pages project `aver` as a Direct Upload project (or with one manual `wrangler pages deploy` as above). Do not connect it to Git, or Cloudflare also builds every branch.
4. Run the workflow from the **Actions** tab, or push a change under `site/`.

To stop deploys, set `SITE_DEPLOY` to anything else or delete it.

### Domains

After the first deployment, add the domains:

1. In the Pages project, open **Custom domains** and add `aver.build` and `www.aver.build`.
2. Redirect `www` to the apex. Pages' `_redirects` matches paths only and does not support domain-level rules, so `_redirects` holds only a note. In the `aver.build` zone, open **Rules > Redirect Rules**, create a rule from the **Redirect from WWW to root** template, and set it to a 301 to `https://aver.build` that keeps the path and query string.
3. Load `https://www.aver.build/receipt.html` and check that it answers 301 with `Location: https://aver.build/receipt.html`, and that `https://aver.build/` sends the headers from `_headers`.

### GitHub Pages

Pages can serve a branch only from `/` or `/docs`, so publish `site/` with a workflow:

1. In the repository, open **Settings > Pages** and set **Source** to **GitHub Actions**.
2. Add `.github/workflows/pages.yml` that runs on pushes to `main` with `actions/checkout`, `actions/upload-pages-artifact` (`path: site`) and `actions/deploy-pages`. The job needs `pages: write` and `id-token: write` permissions.
3. Push. The site appears at `https://<owner>.github.io/<repo>/`.

GitHub Pages ignores `_headers` and `_redirects`; the pages' meta CSPs still apply. Its 404 page is `404.html` only at the site root, so the root-relative links in `404.html` break under a `/<repo>/` path.

## Screenshots

`screenshots/` holds the landing page, the viewer showing the verified example receipt, and the 404 page, each at 1440 px and 375 px wide in light and dark.
