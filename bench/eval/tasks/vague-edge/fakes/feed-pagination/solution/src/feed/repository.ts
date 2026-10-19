import type { Activity } from '../types.ts';
import type { CursorPosition } from './cursor.ts';
import { compareActivities, sortActivities, type SortOrder } from './order.ts';

export interface PageRequest {
  /** Only activities for which this returns true are paged over. */
  where?: (activity: Activity) => boolean;
  order: SortOrder;
  /** Start after this position; omit for the first page. */
  after?: CursorPosition;
  limit: number;
}

export interface PageResult {
  items: Activity[];
  /** True when at least one more matching activity follows the page. */
  hasMore: boolean;
}

export interface ActivityRepository {
  add(activity: Activity): void;
  get(id: string): Activity | undefined;
  delete(id: string): boolean;
  page(request: PageRequest): PageResult;
}

/**
 * In-memory repository used by tests and local development. The Postgres
 * repository in the platform repo implements the same contract.
 */
export class InMemoryActivityRepository implements ActivityRepository {
  #items = new Map<string, Activity>();

  constructor(activities: Iterable<Activity> = []) {
    for (const activity of activities) this.add(activity);
  }

  get size(): number {
    return this.#items.size;
  }

  add(activity: Activity): void {
    if (this.#items.has(activity.id)) throw new Error(`duplicate activity id: ${activity.id}`);
    this.#items.set(activity.id, activity);
  }

  get(id: string): Activity | undefined {
    return this.#items.get(id);
  }

  delete(id: string): boolean {
    return this.#items.delete(id);
  }

  page({ where = () => true, order, after, limit }: PageRequest): PageResult {
    const matching = sortActivities([...this.#items.values()].filter(where), order);
    const rest = after === undefined ? matching : matching.filter((activity) => isAfter(activity, after, order));
    return { items: rest.slice(0, limit), hasMore: rest.length > limit };
  }
}

function isAfter(activity: Activity, position: CursorPosition, order: SortOrder): boolean {
  if (position.id !== undefined) {
    // The feed order is total (createdAt, then id), so the position of the
    // last activity on the page splits it exactly, even within a tie.
    return compareActivities(activity, { createdAt: position.createdAt, id: position.id }, order) > 0;
  }
  // A v1 cursor only knows the timestamp: continue strictly after it.
  return order === 'asc' ? activity.createdAt > position.createdAt : activity.createdAt < position.createdAt;
}
