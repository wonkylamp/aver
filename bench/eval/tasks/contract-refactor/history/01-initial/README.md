# @acme/textkit

String, number, date and collection helpers shared by Acme's services
(checkout, the CMS, the reporting jobs). Import from the package root only:

```ts
import { slugify, formatBytes } from "@acme/textkit";
```

Node 22.18 or later, no dependencies, no build step: `npm test`. The
`examples/` scripts run with `node examples/<name>.ts`.
