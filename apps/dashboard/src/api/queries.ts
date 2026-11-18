import { infiniteQueryOptions, keepPreviousData, queryOptions } from "@tanstack/react-query";

import { ApiError, api, setCsrfToken, unwrap } from "./client";
import { nextPageParam } from "./paging";
import type { paths } from "./schema.gen";
import type { Action, ModuleName } from "./types";
import { moduleSegment } from "./types";

type Query<P extends keyof paths> = paths[P] extends { get: { parameters: { query?: infer Q } } }
  ? NonNullable<Q>
  : never;

export type CaseFilters = Omit<Query<"/api/guilds/{guild_id}/cases">, "cursor">;
export type ModLogFilters = Omit<Query<"/api/guilds/{guild_id}/mod-log">, "cursor" | "limit">;
export type AppealFilters = Omit<Query<"/api/guilds/{guild_id}/appeals">, "cursor">;

const guildPath = (guildId: string) => ({ path: { guild_id: guildId } });

/**
 * Query keys are hierarchical so a write can invalidate everything it might
 * have changed with one prefix, for example every case query in a server.
 */
export const keys = {
  auth: ["auth"] as const,
  guilds: ["guilds"] as const,
  guild: (guildId: string) => ["guild", guildId] as const,
  cases: (guildId: string) => ["guild", guildId, "cases"] as const,
  appeals: (guildId: string) => ["guild", guildId, "appeals"] as const,
  actions: (guildId: string) => ["guild", guildId, "actions"] as const,
  rules: (guildId: string) => ["guild", guildId, "rules"] as const,
  settings: (guildId: string) => ["guild", guildId, "settings"] as const,
  modLog: (guildId: string) => ["guild", guildId, "mod-log"] as const,
  modules: (guildId: string) => ["guild", guildId, "modules"] as const,
  member: ["member"] as const,
};

/**
 * authQuery resolves to the signed-in account, or null when there is no
 * session. It also primes the CSRF token every write needs.
 */
export const authQuery = queryOptions({
  queryKey: keys.auth,
  queryFn: async () => {
    try {
      const me = await unwrap(api.GET("/api/auth/me"));
      setCsrfToken(me.csrf_token);
      return me;
    } catch (error) {
      if (error instanceof ApiError && error.signedOut) return null;
      throw error;
    }
  },
  staleTime: 5 * 60_000,
});

/** guildsQuery lists every server of the account, with or without Quack. */
export const guildsQuery = queryOptions({
  queryKey: keys.guilds,
  queryFn: () => unwrap(api.GET("/api/guilds")).then((r) => r.guilds),
  staleTime: 60_000,
});

export const guildMeQuery = (guildId: string) =>
  queryOptions({
    queryKey: [...keys.guild(guildId), "me"],
    queryFn: () => unwrap(api.GET("/api/guilds/{guild_id}/me", { params: guildPath(guildId) })),
    staleTime: 60_000,
  });

/** healthQuery is what Quack needs fixed in a server: permissions, channels, modules. */
export const healthQuery = (guildId: string) =>
  queryOptions({
    queryKey: [...keys.guild(guildId), "health"],
    queryFn: () => unwrap(api.GET("/api/guilds/{guild_id}/health", { params: guildPath(guildId) })),
    staleTime: 30_000,
  });

export const statisticsQuery = (guildId: string, from: string, to: string) =>
  queryOptions({
    queryKey: [...keys.guild(guildId), "statistics", from, to],
    queryFn: () =>
      unwrap(
        api.GET("/api/guilds/{guild_id}/statistics", {
          params: { ...guildPath(guildId), query: { from, to } },
        }),
      ),
    staleTime: 60_000,
  });

export const settingsQuery = (guildId: string) =>
  queryOptions({
    queryKey: keys.settings(guildId),
    queryFn: () =>
      unwrap(api.GET("/api/guilds/{guild_id}/settings", { params: guildPath(guildId) })),
  });

/** rulesQuery lists active rules, or archived ones with archived set. */
export const rulesQuery = (guildId: string, archived = false) =>
  queryOptions({
    queryKey: [...keys.rules(guildId), "list", archived],
    queryFn: () =>
      unwrap(
        api.GET("/api/guilds/{guild_id}/rules", {
          params: { ...guildPath(guildId), query: { archived } },
        }),
      ).then((r) => r.rules),
    staleTime: 60_000,
  });

export const ruleQuery = (guildId: string, ruleId: string) =>
  queryOptions({
    queryKey: [...keys.rules(guildId), "detail", ruleId],
    queryFn: () =>
      unwrap(
        api.GET("/api/guilds/{guild_id}/rules/{rule_id}", {
          params: { path: { guild_id: guildId, rule_id: ruleId } },
        }),
      ),
  });

/** ruleVersionQuery is one saved version of a rule; versions never change. */
export const ruleVersionQuery = (guildId: string, ruleId: string, version: number) =>
  queryOptions({
    queryKey: [...keys.rules(guildId), "version", ruleId, version],
    queryFn: () =>
      unwrap(
        api.GET("/api/guilds/{guild_id}/rules/{rule_id}/versions/{version}", {
          params: { path: { guild_id: guildId, rule_id: ruleId, version } },
        }),
      ),
    staleTime: Number.POSITIVE_INFINITY,
  });

export const casesQuery = (guildId: string, filters: CaseFilters, cursor?: string) =>
  queryOptions({
    queryKey: [...keys.cases(guildId), "list", filters, cursor ?? null],
    queryFn: () =>
      unwrap(
        api.GET("/api/guilds/{guild_id}/cases", {
          params: { ...guildPath(guildId), query: { ...filters, cursor } },
        }),
      ),
    placeholderData: keepPreviousData,
  });

/** moving reports whether an action is still on its way, so its page keeps refreshing. */
export const moving = (a: Pick<Action, "state">) => a.state === "pending" || a.state === "running";

export const caseQuery = (guildId: string, caseRef: string) =>
  queryOptions({
    queryKey: [...keys.cases(guildId), "detail", caseRef],
    queryFn: () =>
      unwrap(
        api.GET("/api/guilds/{guild_id}/cases/{case_ref}", {
          params: { path: { guild_id: guildId, case_ref: caseRef } },
        }),
      ),
    // Actions and the DM settle within seconds of a case being added, so
    // keep the page live while any of them is still moving.
    refetchInterval: (query) => {
      const c = query.state.data;
      if (!c) return false;
      const dmMoving = c.dm?.state === "pending" || c.dm?.state === "sending";
      const evidenceMoving = c.evidence.some((e) => e.state === "saving");
      return c.actions.some(moving) || dmMoving || evidenceMoving ? 2_000 : false;
    },
  });

/** casePreviewQuery is the level a rule would pick for a member right now. */
export const casePreviewQuery = (guildId: string, memberId: string, ruleId: string) =>
  queryOptions({
    queryKey: [...keys.cases(guildId), "preview", memberId, ruleId],
    queryFn: () =>
      unwrap(
        api.GET("/api/guilds/{guild_id}/cases/preview", {
          params: { ...guildPath(guildId), query: { member_id: memberId, rule_id: ruleId } },
        }),
      ),
    staleTime: 0,
  });

export const memberSummaryQuery = (guildId: string, memberId: string) =>
  queryOptions({
    queryKey: [...keys.cases(guildId), "member", memberId],
    queryFn: () =>
      unwrap(
        api.GET("/api/guilds/{guild_id}/members/{member_id}/summary", {
          params: { path: { guild_id: guildId, member_id: memberId } },
        }),
      ),
  });

/** failedActionsQuery lists failed actions, or the dismissed ones. */
export const failedActionsQuery = (guildId: string, dismissed = false, cursor?: string) =>
  queryOptions({
    queryKey: [...keys.actions(guildId), "failed", dismissed, cursor ?? null],
    queryFn: () =>
      unwrap(
        api.GET("/api/guilds/{guild_id}/actions", {
          params: { ...guildPath(guildId), query: { state: "failed", dismissed, cursor } },
        }),
      ),
    placeholderData: keepPreviousData,
  });

export const appealsQuery = (guildId: string, filters: AppealFilters, cursor?: string) =>
  queryOptions({
    queryKey: [...keys.appeals(guildId), "list", filters, cursor ?? null],
    queryFn: () =>
      unwrap(
        api.GET("/api/guilds/{guild_id}/appeals", {
          params: { ...guildPath(guildId), query: { ...filters, cursor } },
        }),
      ),
    placeholderData: keepPreviousData,
  });

export const appealQuery = (guildId: string, appealId: string) =>
  queryOptions({
    queryKey: [...keys.appeals(guildId), "detail", appealId],
    queryFn: () =>
      unwrap(
        api.GET("/api/guilds/{guild_id}/appeals/{appeal_id}", {
          params: { path: { guild_id: guildId, appeal_id: appealId } },
        }),
      ),
  });

/**
 * modLogQuery pages through the moderation log newest first. Each page asks
 * for entries after the last one seen, so entries written while someone
 * reads never shift the pages.
 */
export const modLogQuery = (guildId: string, filters: ModLogFilters) =>
  infiniteQueryOptions({
    queryKey: [...keys.modLog(guildId), "list", filters],
    queryFn: ({ pageParam }) =>
      unwrap(
        api.GET("/api/guilds/{guild_id}/mod-log", {
          params: { ...guildPath(guildId), query: { ...filters, limit: 50, cursor: pageParam } },
        }),
      ),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: nextPageParam,
    staleTime: 60_000,
  });

/** memberCasesQuery pages through the signed-in member's own cases. */
export const memberCasesQuery = (guildId?: string) =>
  infiniteQueryOptions({
    queryKey: [...keys.member, "cases", guildId ?? null],
    queryFn: ({ pageParam }) =>
      unwrap(
        api.GET("/api/me/cases", {
          params: { query: { guild_id: guildId, limit: 50, cursor: pageParam } },
        }),
      ),
    initialPageParam: undefined as string | undefined,
    getNextPageParam: nextPageParam,
  });

export const memberCaseQuery = (caseId: string) =>
  queryOptions({
    queryKey: [...keys.member, "case", caseId],
    queryFn: () =>
      unwrap(api.GET("/api/me/cases/{case_id}", { params: { path: { case_id: caseId } } })),
  });

export const ticketsModuleQuery = (guildId: string) =>
  queryOptions({
    queryKey: [...keys.modules(guildId), "tickets"],
    queryFn: () =>
      unwrap(api.GET("/api/guilds/{guild_id}/modules/tickets", { params: guildPath(guildId) })),
  });

export const activityLogModuleQuery = (guildId: string) =>
  queryOptions({
    queryKey: [...keys.modules(guildId), "activity_log"],
    queryFn: () =>
      unwrap(
        api.GET("/api/guilds/{guild_id}/modules/activity-log", { params: guildPath(guildId) }),
      ),
  });

export const honeypotModuleQuery = (guildId: string) =>
  queryOptions({
    queryKey: [...keys.modules(guildId), "honeypot"],
    queryFn: () =>
      unwrap(api.GET("/api/guilds/{guild_id}/modules/honeypot", { params: guildPath(guildId) })),
  });

export type TicketListState = "open" | "closed";

/** ticketsQuery is one page of tickets in a state. */
export const ticketsQuery = (guildId: string, state: TicketListState, cursor?: string) =>
  queryOptions({
    queryKey: [...keys.modules(guildId), "tickets", "list", state, cursor ?? null],
    queryFn: () =>
      unwrap(
        api.GET("/api/guilds/{guild_id}/tickets", {
          params: { ...guildPath(guildId), query: { state, cursor } },
        }),
      ),
    placeholderData: keepPreviousData,
  });

export const ticketQuery = (guildId: string, ticketId: string) =>
  queryOptions({
    queryKey: [...keys.modules(guildId), "tickets", "detail", ticketId],
    queryFn: () =>
      unwrap(
        api.GET("/api/guilds/{guild_id}/tickets/{ticket_id}", {
          params: { path: { guild_id: guildId, ticket_id: ticketId } },
        }),
      ),
  });

/**
 * ticketTranscriptQuery is a closed ticket's saved transcript. It is a 404
 * once retention has deleted it, so it is never retried.
 */
export const ticketTranscriptQuery = (guildId: string, ticketId: string) =>
  queryOptions({
    queryKey: [...keys.modules(guildId), "tickets", "transcript", ticketId],
    queryFn: () =>
      unwrap(
        api.GET("/api/guilds/{guild_id}/tickets/{ticket_id}/transcript", {
          params: { path: { guild_id: guildId, ticket_id: ticketId } },
        }),
      ),
    retry: false,
    staleTime: 5 * 60_000,
  });

/** moduleUrl is a module's segment for API URLs. */
export const moduleUrl = (name: ModuleName) => moduleSegment[name];
