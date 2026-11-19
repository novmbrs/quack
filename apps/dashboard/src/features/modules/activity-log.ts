/**
 * ActivityEvent is a Discord event the server activity log can post to a
 * channel (design §13.3.1). Each is a key of the settings' routes map.
 */
export type ActivityEvent =
  | "message_edited"
  | "message_deleted"
  | "messages_bulk_deleted"
  | "member_joined"
  | "member_left"
  | "member_banned"
  | "member_unbanned"
  | "server_changed"
  | "channel_changed";

/** activityEventGroups lists every event under a heading, in the order the page shows them. */
export const activityEventGroups: {
  title: string;
  events: { key: ActivityEvent; label: string; hint: string }[];
}[] = [
  {
    title: "Messages",
    events: [
      {
        key: "message_edited",
        label: "Edited messages",
        hint: "What the message said before and after.",
      },
      {
        key: "message_deleted",
        label: "Deleted messages",
        hint: "Who sent it and what it said, when Quack had seen it.",
      },
      {
        key: "messages_bulk_deleted",
        label: "Bulk deletions",
        hint: "Many messages deleted at once, like a purge.",
      },
    ],
  },
  {
    title: "Members",
    events: [
      { key: "member_joined", label: "Joins", hint: "A member joined the server." },
      { key: "member_left", label: "Leaves", hint: "A member left the server or was removed." },
    ],
  },
  {
    title: "Bans",
    events: [
      {
        key: "member_banned",
        label: "Bans",
        hint: "Bans made outside Quack. Needs View Audit Log for Quack.",
      },
      {
        key: "member_unbanned",
        label: "Unbans",
        hint: "Unbans made outside Quack. Needs View Audit Log for Quack.",
      },
    ],
  },
  {
    title: "Server",
    events: [
      { key: "server_changed", label: "Server changes", hint: "The server's name and settings." },
      {
        key: "channel_changed",
        label: "Channel changes",
        hint: "Channels created, changed, or deleted.",
      },
    ],
  },
];

/** activityEvents is every event that can be routed. */
export const activityEvents: ActivityEvent[] = activityEventGroups.flatMap((g) =>
  g.events.map((e) => e.key),
);

/** Routes maps each event to its channel ID, "" when it isn't posted. */
export type Routes = Record<ActivityEvent, string>;

/** routesFrom reads the settings' routes map, ignoring unknown keys. */
export function routesFrom(routes: Record<string, string> | null | undefined): Routes {
  const out = {} as Routes;
  for (const event of activityEvents) out[event] = routes?.[event]?.trim() ?? "";
  return out;
}

/** routesToSave is the routes map to save; events that aren't posted are left out. */
export function routesToSave(routes: Routes): Record<string, string> {
  const out: Record<string, string> = {};
  for (const event of activityEvents) if (routes[event]) out[event] = routes[event];
  return out;
}

/** sameRoutes reports whether two routings send every event to the same place. */
export function sameRoutes(a: Routes, b: Routes): boolean {
  return activityEvents.every((e) => a[e] === b[e]);
}

/**
 * missingDestinations finds routed channels that aren't in the server's
 * channel list, which almost always means they were deleted, with the
 * events each one would carry.
 */
export function missingDestinations(
  routes: Routes,
  known: ReadonlySet<string>,
): { channelId: string; events: ActivityEvent[] }[] {
  const missing = new Map<string, ActivityEvent[]>();
  for (const event of activityEvents) {
    const id = routes[event];
    if (!id || known.has(id)) continue;
    missing.set(id, [...(missing.get(id) ?? []), event]);
  }
  return [...missing].map(([channelId, events]) => ({ channelId, events }));
}

/** withoutEvents stops posting the given events anywhere. */
export function withoutEvents(routes: Routes, events: readonly ActivityEvent[]): Routes {
  const out = { ...routes };
  for (const event of events) out[event] = "";
  return out;
}

/** eventLabel is an event's label on the page, such as "Edited messages". */
export function eventLabel(event: ActivityEvent): string {
  for (const group of activityEventGroups)
    for (const e of group.events) if (e.key === event) return e.label;
  return event;
}
