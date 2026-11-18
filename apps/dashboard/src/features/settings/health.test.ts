import { describe, expect, it } from "vite-plus/test";

import type { Health } from "~/api/types";

import { healthChecklist, healthy, permissionList } from "./health";

const off = { enabled: false, problems: [] };

const base: Health = {
  quack_in_server: true,
  missing_permissions: [],
  channels: [],
  modules: { tickets: off, activity_log: off, honeypot: off },
};

describe("healthChecklist", () => {
  it("passes a healthy server", () => {
    const checks = healthChecklist(base);
    expect(checks.map((c) => [c.key, c.state])).toEqual([
      ["quack", "ok"],
      ["permissions", "ok"],
    ]);
    expect(healthy(checks)).toBe(true);
  });

  it("names missing permissions the way Discord does", () => {
    const checks = healthChecklist({
      ...base,
      missing_permissions: ["moderate_members", "ban_members"],
    });
    expect(checks[1]?.state).toBe("problem");
    expect(checks[1]?.detail).toContain("Timeout Members and Ban Members");
    expect(healthy(checks)).toBe(false);
  });

  it("shows channel and module problems with the API's messages", () => {
    const checks = healthChecklist({
      ...base,
      channels: [
        {
          setting: "log_channel_id",
          channel_id: "1",
          problems: [{ code: "everyone_can_view", message: "Everyone can view this channel." }],
        },
        { setting: "evidence_channel_id", channel_id: "2", problems: [] },
      ],
      modules: {
        ...base.modules,
        tickets: { enabled: true, problems: [{ code: "x", message: "Choose a staff channel." }] },
        honeypot: { enabled: true, problems: [] },
      },
    });
    expect(checks.map((c) => [c.label, c.state, c.detail])).toEqual([
      ["Quack is in the server", "ok", "Quack is a member of this server."],
      ["Permissions", "ok", "Quack's role has every permission Quack asks for."],
      ["Log channel", "problem", "Everyone can view this channel."],
      ["Evidence channel", "ok", "Only staff can see it, and Quack can post there."],
      ["Tickets", "problem", "Choose a staff channel."],
      ["Honeypot", "ok", "On and working."],
    ]);
  });

  it("stops when Quack isn't in the server", () => {
    const checks = healthChecklist({ ...base, quack_in_server: false });
    expect(checks.map((c) => [c.key, c.state])).toEqual([["quack", "problem"]]);
  });
});

describe("permissionList", () => {
  it("joins names", () => {
    expect(permissionList(["kick_members"])).toBe("Kick Members");
    expect(permissionList(["kick_members", "ban_members", "manage_guild"])).toBe(
      "Kick Members, Ban Members, and Manage Server",
    );
  });
});
