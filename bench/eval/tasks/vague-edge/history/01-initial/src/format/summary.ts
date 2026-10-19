import type { Activity, Verb } from '../types.ts';
import type { UserDirectory } from '../users/directory.ts';

const PHRASES: Record<Verb, string> = {
  created: 'created',
  updated: 'updated',
  commented: 'commented on',
  shared: 'shared',
  mentioned: 'mentioned you in',
  deleted: 'deleted',
};

const MAX_TITLE = 60;

/** One-line, plain-text description such as: Ana Lima commented on task "Fix login". */
export function summarize(activity: Activity, users: UserDirectory): string {
  const actor = users.displayName(activity.actorId);
  const { type, title } = activity.object;
  const target = title === undefined || title.trim() === '' ? `a ${type}` : `${type} "${truncate(title.trim())}"`;
  return `${actor} ${PHRASES[activity.verb]} ${target}`;
}

export function truncate(text: string, max = MAX_TITLE): string {
  const chars = Array.from(text);
  if (chars.length <= max) return text;
  return `${chars.slice(0, max - 1).join('').trimEnd()}…`;
}
