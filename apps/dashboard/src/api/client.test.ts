import { describe, expect, it } from "vite-plus/test";

import { ApiError, loginUrl, toApiError, write, type WriteInit } from "./client";
import { back, cursorOf, firstPage, forward, nextPageParam } from "./paging";

const json = (status: number, body: unknown, headers: Record<string, string> = {}) =>
  new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json", ...headers },
  });

/** fake records the Idempotency-Key of every call and answers with the given results in turn. */
function fake(results: (() => Promise<{ data?: unknown; error?: unknown; response: Response }>)[]) {
  const keys: string[] = [];
  let i = 0;
  const call = (init: WriteInit) => {
    keys.push(init.headers["Idempotency-Key"]);
    const next = results[Math.min(i++, results.length - 1)]!;
    return next();
  };
  return { keys, call };
}

const ok = () => Promise.resolve({ data: { done: true }, response: json(200, { done: true }) });
const unavailable = () =>
  Promise.resolve({
    error: { error: { code: "unavailable", message: "Quack is busy.", trace_id: "abc" } },
    response: json(503, {}),
  });
const offline = () => Promise.reject(new TypeError("fetch failed"));

describe("write", () => {
  it("reuses one Idempotency-Key across automatic retries", async () => {
    const { keys, call } = fake([offline, unavailable, ok]);
    await expect(write(call, { delay: () => 0 })).resolves.toEqual({ done: true });
    expect(keys).toHaveLength(3);
    expect(new Set(keys).size).toBe(1);
    expect(keys[0]!.length).toBeGreaterThanOrEqual(8);
  });

  it("uses a fresh key for each new write", async () => {
    const first = fake([ok]);
    const second = fake([ok]);
    await write(first.call);
    await write(second.call);
    expect(first.keys[0]).not.toBe(second.keys[0]);
  });

  it("stops after the retry budget and throws the last error", async () => {
    const { keys, call } = fake([unavailable]);
    const error = await write(call, { delay: () => 0, retries: 2 }).catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ApiError);
    expect((error as ApiError).status).toBe(503);
    expect(keys).toHaveLength(3);
  });

  it("never retries a refusal", async () => {
    const conflict = () =>
      Promise.resolve({
        error: { error: { code: "rule_changed", message: "Someone saved first." } },
        response: json(409, {}),
      });
    const { keys, call } = fake([conflict, ok]);
    await expect(write(call, { delay: () => 0 })).rejects.toMatchObject({ code: "rule_changed" });
    expect(keys).toHaveLength(1);
  });
});

describe("toApiError", () => {
  it("keeps the envelope's message, fields, and trace", () => {
    const error = toApiError(json(400, {}), {
      error: {
        code: "validation_failed",
        message: "Check the marked fields and save again.",
        trace_id: "4bf92f3577b34da6a3ce929d0e0e4736",
        fields: [
          { field: "levels[1].from_case_count", code: "level_order", message: "Levels go up." },
          { field: "name", code: "name_length", message: "Give the rule a name." },
        ],
      },
    });
    expect(error.status).toBe(400);
    expect(error.code).toBe("validation_failed");
    expect(error.message).toBe("Check the marked fields and save again.");
    expect(error.traceId).toBe("4bf92f3577b34da6a3ce929d0e0e4736");
    expect(error.field("name")).toBe("Give the rule a name.");
    expect(error.field("levels")).toBe("Levels go up.");
    expect(error.field("levels[1]")).toBe("Levels go up.");
    expect(error.field("reason")).toBeUndefined();
    expect(error.unexpected).toBe(false);
  });

  it("reads Retry-After and module problems", () => {
    const limited = toApiError(json(429, {}, { "Retry-After": "12" }), {
      error: { code: "rate_limited", message: "Slow down." },
    });
    expect(limited.retryAfter).toBe(12);
    const notReady = toApiError(json(409, {}), {
      error: {
        code: "module_not_ready",
        message: "Tickets are still off.",
        problems: [{ code: "ticket_queue_missing", message: "Choose a staff channel." }],
      },
    });
    expect(notReady.problems.map((p) => p.code)).toEqual(["ticket_queue_missing"]);
  });

  it("falls back to a sentence when the body isn't an envelope", () => {
    const error = toApiError(json(502, {}), "<html>bad gateway</html>");
    expect(error.code).toBe("internal_error");
    expect(error.message).toMatch(/^Quack couldn't/);
    expect(error.unexpected).toBe(true);
  });

  it("treats 401 as signed out", () => {
    const error = toApiError(json(401, {}), {
      error: { code: "session_expired", message: "Your session ended. Sign in again." },
    });
    expect(error.signedOut).toBe(true);
  });
});

describe("loginUrl", () => {
  it("passes the page to come back to", () => {
    expect(loginUrl("/guilds/1/cases?x=1")).toBe(
      "/api/auth/login?redirect_to=%2Fguilds%2F1%2Fcases%3Fx%3D1",
    );
  });
});

describe("cursor paging", () => {
  it("goes forward with next_cursor and back again", () => {
    let trail = firstPage;
    expect(cursorOf(trail)).toBeUndefined();
    trail = forward(trail, "c2");
    trail = forward(trail, "c3");
    expect(cursorOf(trail)).toBe("c3");
    trail = back(trail);
    expect(cursorOf(trail)).toBe("c2");
    trail = back(trail);
    trail = back(trail);
    expect(cursorOf(trail)).toBeUndefined();
    // Going forward again from an earlier page replaces what came after it.
    trail = forward(trail, "other");
    expect(trail.cursors).toEqual([undefined, "other"]);
  });

  it("stays on the last page", () => {
    const trail = forward(firstPage, null);
    expect(trail).toBe(firstPage);
  });

  it("feeds infinite queries", () => {
    expect(nextPageParam({ next_cursor: "abc" })).toBe("abc");
    expect(nextPageParam({ next_cursor: null })).toBeUndefined();
    expect(nextPageParam({})).toBeUndefined();
  });
});
