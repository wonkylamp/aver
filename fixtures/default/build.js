// The build the tests need: dist/ is not committed, as in most packages.
const fs = require('node:fs');

fs.mkdirSync('dist', { recursive: true });
fs.copyFileSync('src/index.js', 'dist/index.js');
