import { useState } from "react";

/**
 * Lists in the API page by cursor (design §14.1): each page carries
 * next_cursor, null on the last page, and there are no totals or offsets.
 * CursorTrail remembers the cursor of every page seen so far, so a table
 * can go back as well as forward.
 */
export type CursorTrail = {
  /** The cursor of each page reached so far; the first page has none. */
  cursors: (string | undefined)[];
  /** Which of those pages is showing. */
  index: number;
};

export const firstPage: CursorTrail = { cursors: [undefined], index: 0 };

/** cursorOf is the cursor to request for the page showing now. */
export function cursorOf(trail: CursorTrail): string | undefined {
  return trail.cursors[trail.index];
}

/** forward moves to the next page; next is the showing page's next_cursor. */
export function forward(trail: CursorTrail, next: string | null | undefined): CursorTrail {
  if (!next) return trail;
  const cursors = trail.cursors.slice(0, trail.index + 1);
  cursors.push(next);
  return { cursors, index: trail.index + 1 };
}

/** back returns to the page before, keeping the trail for going forward again. */
export function back(trail: CursorTrail): CursorTrail {
  return trail.index === 0 ? trail : { ...trail, index: trail.index - 1 };
}

/** nextPageParam is getNextPageParam for infinite queries over a cursor list. */
export function nextPageParam(page: { next_cursor?: string | null }): string | undefined {
  return page.next_cursor || undefined;
}

/**
 * useCursorPager keeps a CursorTrail for one list. Changing resetKey (the
 * list's filters serialized) starts again from the first page, since a
 * cursor only works with the filters it came from.
 */
export function useCursorPager(resetKey: string) {
  const [state, setState] = useState({ key: resetKey, trail: firstPage });
  const trail = state.key === resetKey ? state.trail : firstPage;
  const set = (t: CursorTrail) => setState({ key: resetKey, trail: t });
  return {
    cursor: cursorOf(trail),
    page: trail.index + 1,
    hasPrevious: trail.index > 0,
    next: (nextCursor: string | null | undefined) => set(forward(trail, nextCursor)),
    previous: () => set(back(trail)),
    reset: () => set(firstPage),
  };
}
