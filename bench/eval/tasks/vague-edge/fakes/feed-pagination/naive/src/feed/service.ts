import { InvalidParameterError } from '../errors.ts';
import { canView } from '../permissions.ts';
import type { Activity, Viewer } from '../types.ts';
import { decodeCursor, encodeCursor } from './cursor.ts';
import { matchesFilter, type FeedFilter } from './filters.ts';
import type { SortOrder } from './order.ts';
import type { ActivityRepository } from './repository.ts';

export const DEFAULT_LIMIT = 20;
export const MAX_LIMIT = 100;

export interface FeedQuery {
  /** Page size, 1 to MAX_LIMIT. Default DEFAULT_LIMIT. */
  limit?: number;
  /** nextCursor from the previous page. */
  cursor?: string;
  /** Default 'desc' (newest first). */
  order?: SortOrder;
  filter?: FeedFilter;
}

export interface FeedPage {
  items: Activity[];
  /** Pass as `cursor` to get the next page; null on the last page. */
  nextCursor: string | null;
}

export class FeedService {
  #repository: ActivityRepository;

  constructor(repository: ActivityRepository) {
    this.#repository = repository;
  }

  list(viewer: Viewer, query: FeedQuery = {}): FeedPage {
    const limit = query.limit ?? DEFAULT_LIMIT;
    if (!Number.isInteger(limit) || limit < 1 || limit > MAX_LIMIT) {
      throw new InvalidParameterError('limit', `limit must be an integer from 1 to ${MAX_LIMIT}`);
    }
    const order = query.order ?? 'desc';
    const after = query.cursor === undefined ? undefined : decodeCursor(query.cursor);
    const filter = query.filter ?? {};

    const { items, hasMore } = this.#repository.page({
      where: (activity) => canView(viewer, activity) && matchesFilter(activity, filter),
      order,
      after,
      limit,
    });

    const last = items.at(-1);
    return {
      items,
      nextCursor: hasMore && last !== undefined ? encodeCursor({ createdAt: last.createdAt, id: last.id }) : null,
    };
  }
}
