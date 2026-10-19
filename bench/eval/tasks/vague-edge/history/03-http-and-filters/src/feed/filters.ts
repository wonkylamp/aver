import { InvalidParameterError } from '../errors.ts';
import { VERBS, type Activity, type Verb } from '../types.ts';

export interface FeedFilter {
  /** Only these verbs. Omit or leave empty for all verbs. */
  verbs?: readonly Verb[];
  /** Only activities by this user. */
  actorId?: string;
}

export function matchesFilter(activity: Activity, filter: FeedFilter = {}): boolean {
  if (filter.verbs !== undefined && filter.verbs.length > 0 && !filter.verbs.includes(activity.verb)) return false;
  if (filter.actorId !== undefined && activity.actorId !== filter.actorId) return false;
  return true;
}

/** Parses "commented,shared" into verbs. Blank means no verb filter. */
export function parseVerbList(raw: string | null | undefined): Verb[] | undefined {
  if (raw === null || raw === undefined || raw.trim() === '') return undefined;
  const verbs: Verb[] = [];
  for (const part of raw.split(',')) {
    const verb = part.trim();
    if (!isVerb(verb)) {
      throw new InvalidParameterError('verb', `unknown verb "${verb}"; expected one of ${VERBS.join(', ')}`);
    }
    if (!verbs.includes(verb)) verbs.push(verb);
  }
  return verbs;
}

function isVerb(value: string): value is Verb {
  return (VERBS as readonly string[]).includes(value);
}
