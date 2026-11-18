import { describe, expect, it } from "vite-plus/test";

import {
  activityEvents,
  eventLabel,
  missingDestinations,
  routesFrom,
  routesToSave,
  sameRoutes,
  withoutEvents,
} from "./activity-log";

describe("routesFrom and routesToSave", () => {
  it("covers every event and drops unknown keys", () => {
    const routes = routesFrom({ message_edited: "1", something_else: "2" });
    expect(Object.keys(routes)).toEqual(activityEvents);
    expect(routes.message_edited).toBe("1");
    expect(routes.member_joined).toBe("");
    expect(routesToSave(routes)).toEqual({ message_edited: "1" });
  });

  it("handles a server that never routed anything", () => {
    expect(routesToSave(routesFrom(null))).toEqual({});
  });

  it("knows all nine events", () => {
    expect(activityEvents).toHaveLength(9);
    expect(eventLabel("messages_bulk_deleted")).toBe("Bulk deletions");
  });
});

describe("sameRoutes", () => {
  it("compares every event", () => {
    const a = routesFrom({ message_edited: "1" });
    expect(sameRoutes(a, routesFrom({ message_edited: "1" }))).toBe(true);
    expect(sameRoutes(a, routesFrom({ message_edited: "1", member_banned: "1" }))).toBe(false);
  });
});

describe("missingDestinations", () => {
  it("groups the events routed to each unknown channel", () => {
    const routes = routesFrom({
      message_edited: "gone",
      message_deleted: "gone",
      member_joined: "here",
      member_banned: "also-gone",
    });
    const missing = missingDestinations(routes, new Set(["here"]));
    expect(missing).toEqual([
      { channelId: "gone", events: ["message_edited", "message_deleted"] },
      { channelId: "also-gone", events: ["member_banned"] },
    ]);
    expect(routesToSave(withoutEvents(routes, missing[0]!.events))).toEqual({
      member_joined: "here",
      member_banned: "also-gone",
    });
  });

  it("is empty when every channel exists", () => {
    expect(missingDestinations(routesFrom({ member_joined: "here" }), new Set(["here"]))).toEqual(
      [],
    );
  });
});
