/**
 * Ids are a prefix plus a zero-padded base-36 counter, so ids from one
 * generator sort as strings in creation order.
 */
export function createIdGenerator(prefix: string, start = 0): () => string {
  if (!/^[a-z]+$/.test(prefix)) throw new Error(`invalid id prefix: ${prefix}`);
  let next = start;
  return () => {
    const id = `${prefix}_${next.toString(36).padStart(8, '0')}`;
    next++;
    return id;
  };
}

export function hasPrefix(id: string, prefix: string): boolean {
  return id.startsWith(`${prefix}_`);
}
