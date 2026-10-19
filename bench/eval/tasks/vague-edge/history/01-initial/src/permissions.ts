import type { Activity, Viewer } from './types.ts';

export function canView(viewer: Viewer, activity: Activity): boolean {
  switch (activity.visibility) {
    case 'public':
      return true;
    case 'team':
      return viewer.teamIds.includes(activity.teamId);
    case 'private':
      return viewer.userId === activity.actorId;
  }
}

/** Activities a viewer may see, in their original order. */
export function visibleTo(viewer: Viewer, activities: Iterable<Activity>): Activity[] {
  const visible: Activity[] = [];
  for (const activity of activities) if (canView(viewer, activity)) visible.push(activity);
  return visible;
}
