import type { Activity } from '../types.ts';

/** 'desc' is newest first (the feed default); 'asc' is oldest first. */
export type SortOrder = 'asc' | 'desc';

export const SORT_ORDERS: readonly SortOrder[] = ['asc', 'desc'];

type Sortable = Pick<Activity, 'createdAt' | 'id'>;

/**
 * Feed order: by createdAt, then by id so that activities sharing a timestamp
 * still have one fixed order. 'desc' is the exact reverse of 'asc'.
 */
export function compareActivities(a: Sortable, b: Sortable, order: SortOrder): number {
  let result = a.createdAt - b.createdAt;
  if (result === 0) result = a.id < b.id ? -1 : a.id > b.id ? 1 : 0;
  return order === 'asc' ? result : -result;
}

export function sortActivities<T extends Sortable>(items: readonly T[], order: SortOrder): T[] {
  return [...items].sort((a, b) => compareActivities(a, b, order));
}
