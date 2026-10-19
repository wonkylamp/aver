import { InvalidCursorError, InvalidParameterError } from '../errors.ts';
import { parseVerbList } from '../feed/filters.ts';
import type { FeedService } from '../feed/service.ts';
import { summarize } from '../format/summary.ts';
import type { RateLimiter } from '../rate-limit.ts';
import type { Viewer } from '../types.ts';
import type { UserDirectory } from '../users/directory.ts';
import { optionalString, parseLimit, parseOrder } from './params.ts';
import { errorResponse, json, type HttpResponse } from './response.ts';

export interface FeedRequest {
  /** Null when the request is not authenticated. */
  viewer: Viewer | null;
  query: URLSearchParams;
}

export interface FeedHandlerDeps {
  feed: FeedService;
  users: UserDirectory;
  /** Per-user limit. Omit to disable rate limiting. */
  limiter?: RateLimiter;
}

/** GET /v1/feed. See docs/api.md. */
export function createFeedHandler({ feed, users, limiter }: FeedHandlerDeps): (request: FeedRequest) => HttpResponse {
  return (request) => {
    const { viewer, query } = request;
    if (viewer === null) return errorResponse(401, 'unauthenticated');

    if (limiter !== undefined && !limiter.tryTake(viewer.userId)) {
      const seconds = Math.max(1, Math.ceil(limiter.retryAfter(viewer.userId) / 1000));
      return { ...errorResponse(429, 'rate_limited'), headers: { 'content-type': 'application/json; charset=utf-8', 'retry-after': String(seconds) } };
    }

    try {
      const page = feed.list(viewer, {
        limit: parseLimit(query.get('limit')),
        order: parseOrder(query.get('order')),
        cursor: optionalString(query.get('cursor')),
        filter: {
          verbs: parseVerbList(query.get('verb')),
          actorId: optionalString(query.get('actor')),
        },
      });
      return json(200, {
        items: page.items.map((activity) => ({
          id: activity.id,
          verb: activity.verb,
          actorId: activity.actorId,
          object: activity.object,
          summary: summarize(activity, users),
          createdAt: new Date(activity.createdAt).toISOString(),
        })),
        nextCursor: page.nextCursor,
      });
    } catch (error) {
      if (error instanceof InvalidCursorError) return errorResponse(400, 'invalid_cursor');
      if (error instanceof InvalidParameterError) {
        return errorResponse(400, 'invalid_parameter', { parameter: error.parameter, message: error.message });
      }
      throw error;
    }
  };
}
