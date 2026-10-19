import { createIdGenerator } from '../ids.ts';
import type { Activity, Viewer } from '../types.ts';

export const T0 = Date.UTC(2024, 0, 15, 9, 0, 0);

/**
 * Builds activities one minute apart, in creation order, with ids from a
 * fresh generator. Override any field per activity.
 */
export function activityFactory(start = T0, step = 60_000): (overrides?: Partial<Activity>) => Activity {
  const nextId = createIdGenerator('act');
  let at = start;
  return (overrides = {}) => {
    const activity: Activity = {
      id: nextId(),
      teamId: 'team_a',
      actorId: 'usr_1',
      verb: 'updated',
      object: { type: 'doc', id: 'doc_1', title: 'Roadmap' },
      createdAt: at,
      visibility: 'team',
      ...overrides,
    };
    at += step;
    return activity;
  };
}

export const teamMember: Viewer = { userId: 'usr_2', teamIds: ['team_a'] };
