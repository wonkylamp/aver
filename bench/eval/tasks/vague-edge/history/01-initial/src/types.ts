export const VERBS = ['created', 'updated', 'commented', 'shared', 'mentioned', 'deleted'] as const;
export type Verb = (typeof VERBS)[number];

export type ObjectType = 'doc' | 'task' | 'comment';

/**
 * Who can see an activity:
 * - public: everyone signed in
 * - team: members of the activity's team
 * - private: only the actor
 */
export type Visibility = 'public' | 'team' | 'private';

export interface ActivityObject {
  type: ObjectType;
  id: string;
  title?: string;
}

export interface Activity {
  /** Unique, assigned by the id generator, e.g. "act_0000002s". */
  id: string;
  teamId: string;
  actorId: string;
  verb: Verb;
  object: ActivityObject;
  /** Milliseconds since the Unix epoch. Not unique: bulk imports share one timestamp. */
  createdAt: number;
  visibility: Visibility;
}

export interface Viewer {
  userId: string;
  teamIds: readonly string[];
}

export interface User {
  id: string;
  name: string;
  handle: string;
  deactivated?: boolean;
}
