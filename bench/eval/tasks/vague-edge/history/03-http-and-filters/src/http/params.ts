import { InvalidParameterError } from '../errors.ts';
import { SORT_ORDERS, type SortOrder } from '../feed/order.ts';

/** Absent or blank means "use the default". */
export function parseLimit(raw: string | null): number | undefined {
  if (raw === null || raw.trim() === '') return undefined;
  if (!/^\d+$/.test(raw.trim())) throw new InvalidParameterError('limit', 'limit must be a whole number');
  return Number(raw.trim());
}

export function parseOrder(raw: string | null): SortOrder | undefined {
  if (raw === null || raw.trim() === '') return undefined;
  const order = raw.trim().toLowerCase();
  if (!(SORT_ORDERS as readonly string[]).includes(order)) {
    throw new InvalidParameterError('order', 'order must be "asc" or "desc"');
  }
  return order as SortOrder;
}

export function optionalString(raw: string | null): string | undefined {
  return raw === null || raw === '' ? undefined : raw;
}
