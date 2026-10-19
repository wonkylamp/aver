# tempo-kit

Small timing utilities for Node services: `debounce`, `throttle` and `sleep`,
all driven by an injectable `Clock`.

```ts
import { debounce } from './src/index.ts';

const save = debounce((doc: Doc) => store.write(doc), 200);
editor.on('change', (doc) => save(doc));
```

## Clocks

Every utility takes an optional `clock`. Production code uses the default
`systemClock`. Tests pass a `ManualClock` and move time by hand, so they never
sleep and never depend on how busy the machine is:

```ts
const clock = new ManualClock();
const save = debounce(write, 200, { clock });
save(doc);
clock.advance(200); // write runs here
```

## Development

Requires Node 22.18 or later (TypeScript runs through Node's type stripping;
there is no build step and there are no dependencies).

```sh
npm test
```
