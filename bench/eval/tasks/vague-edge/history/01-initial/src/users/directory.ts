import type { User } from '../types.ts';

export class UserDirectory {
  #byId = new Map<string, User>();
  #byHandle = new Map<string, User>();

  constructor(users: Iterable<User> = []) {
    for (const user of users) this.add(user);
  }

  add(user: User): void {
    const handle = normalizeHandle(user.handle);
    if (this.#byId.has(user.id)) throw new Error(`duplicate user id: ${user.id}`);
    if (this.#byHandle.has(handle)) throw new Error(`duplicate handle: @${handle}`);
    const stored = { ...user, handle };
    this.#byId.set(user.id, stored);
    this.#byHandle.set(handle, stored);
  }

  get(id: string): User | undefined {
    return this.#byId.get(id);
  }

  byHandle(handle: string): User | undefined {
    return this.#byHandle.get(normalizeHandle(handle));
  }

  /** Name shown in the feed. Deactivated and unknown users get a neutral label. */
  displayName(id: string): string {
    const user = this.#byId.get(id);
    if (user === undefined) return 'Unknown user';
    if (user.deactivated) return 'Former member';
    return user.name;
  }

  /** Active users whose handle or name starts with `prefix`, for @-mention autocomplete. */
  search(prefix: string, limit = 10): User[] {
    const needle = prefix.replace(/^@/, '').toLowerCase();
    if (needle === '') return [];
    const matches: User[] = [];
    for (const user of this.#byId.values()) {
      if (user.deactivated) continue;
      if (user.handle.startsWith(needle) || user.name.toLowerCase().startsWith(needle)) matches.push(user);
    }
    matches.sort((a, b) => a.handle.localeCompare(b.handle));
    return matches.slice(0, limit);
  }
}

function normalizeHandle(handle: string): string {
  return handle.replace(/^@/, '').toLowerCase();
}
