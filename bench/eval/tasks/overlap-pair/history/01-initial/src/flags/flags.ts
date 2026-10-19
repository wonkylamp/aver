export interface Flags {
  isEnabled(name: string): boolean;
  list(): string[];
}

// Flag names are case-insensitive and surrounding whitespace is ignored, so
// "New-Dashboard " in a config file switches on "new-dashboard".
export function createFlags(enabled: string[]): Flags {
  const on = new Set(enabled.map(normalize).filter((name) => name !== ""));
  return {
    isEnabled: (name) => on.has(normalize(name)),
    list: () => [...on].sort(),
  };
}

function normalize(name: string): string {
  return name.trim().toLowerCase();
}
