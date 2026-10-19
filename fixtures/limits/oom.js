// Allocates and touches memory until the kernel kills the process.
const held = [];
for (;;) {
  held.push(Buffer.alloc(16 * 1024 * 1024, 1));
}
