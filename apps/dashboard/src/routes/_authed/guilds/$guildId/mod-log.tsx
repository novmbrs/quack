import { useInfiniteQuery } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { ScrollText, X } from "lucide-react";

import { cx } from "~/lib/cx";
import { modLogQuery } from "~/api/queries";
import { ModLogEntry } from "~/features/mod-log/ModLogEntry";
import {
  filtersOf,
  kindGroups,
  type ModLogSearch,
  parseKind,
  parseResult,
} from "~/features/mod-log/kinds";
import { MemberPicker } from "~/features/people/MemberPicker";
import { Button } from "~/ui/Button";
import { Select, TextInput } from "~/ui/Field";
import { Page } from "~/ui/Page";
import { Segmented } from "~/ui/Segmented";
import { Empty, ErrorState, SkeletonRows } from "~/ui/States";
import s from "./mod-log.module.css";

export const Route = createFileRoute("/_authed/guilds/$guildId/mod-log")({
  validateSearch: (raw: Record<string, unknown>): ModLogSearch => ({
    kind: parseKind(raw.kind),
    result: parseResult(raw.result),
    actor: id(raw.actor),
    member: id(raw.member),
    case: caseNumber(raw.case),
    rule: typeof raw.rule === "string" && raw.rule ? raw.rule : undefined,
    from: day(raw.from),
    to: day(raw.to),
  }),
  loaderDeps: ({ search }) => search,
  loader: ({ context, params, deps }) =>
    context.queryClient.ensureInfiniteQueryData(modLogQuery(params.guildId, filtersOf(deps))),
  component: ModLog,
});

function ModLog() {
  const { guildId } = Route.useParams();
  const search = Route.useSearch();
  const navigate = useNavigate({ from: Route.fullPath });
  const log = useInfiniteQuery(modLogQuery(guildId, filtersOf(search)));

  const set = (patch: Partial<ModLogSearch>) =>
    void navigate({ search: (prev) => ({ ...prev, ...patch }), replace: true });

  const entries = log.data?.pages.flatMap((p) => p.entries) ?? [];
  const filtered = Object.values(search).some((v) => v !== undefined && v !== "");

  return (
    <Page
      icon={<ScrollText size={22} />}
      title="Moderation log"
      topic="Who did what in this server: staff and Quack. Entries are never edited or deleted."
    >
      <div className={s.filters}>
        <div className={s.line}>
          <Segmented
            label="Result"
            value={search.result ?? "all"}
            onChange={(v) => set({ result: v === "all" ? undefined : v })}
            options={[
              { value: "all", label: "All" },
              { value: "ok", label: "Done" },
              { value: "failed", label: "Failed" },
              { value: "refused", label: "Refused" },
            ]}
          />
          <Select
            aria-label="Kind of entry"
            value={search.kind ?? ""}
            onChange={(e) => set({ kind: parseKind(e.currentTarget.value) })}
            className={s.action}
          >
            <option value="">Every kind of entry</option>
            {kindGroups.map((g) => (
              <optgroup key={g.label} label={g.label}>
                {g.kinds.map((k) => (
                  <option key={k.value} value={k.value}>
                    {k.label}
                  </option>
                ))}
              </optgroup>
            ))}
          </Select>
          {search.case ? (
            <Button
              variant="secondary"
              size="sm"
              icon={<X size={14} />}
              onClick={() => set({ case: undefined })}
            >
              Case #{search.case}
            </Button>
          ) : null}
          {search.rule ? (
            <Button
              variant="secondary"
              size="sm"
              icon={<X size={14} />}
              onClick={() => set({ rule: undefined })}
            >
              One rule
            </Button>
          ) : null}
        </div>
        <div className={s.line}>
          <div className={s.person}>
            <MemberPicker
              guildId={guildId}
              value={search.actor ?? null}
              onChange={(v) => set({ actor: v ?? undefined })}
              placeholder="Done by"
            />
          </div>
          <div className={s.person}>
            <MemberPicker
              guildId={guildId}
              value={search.member ?? null}
              onChange={(v) => set({ member: v ?? undefined })}
              placeholder="About member"
            />
          </div>
          <label className={s.dates}>
            <span className={s.dateLabel}>From</span>
            <TextInput
              type="date"
              aria-label="From date"
              value={search.from ?? ""}
              max={search.to}
              onChange={(e) => set({ from: e.currentTarget.value || undefined })}
              className={s.date}
            />
          </label>
          <label className={s.dates}>
            <span className={s.dateLabel}>To</span>
            <TextInput
              type="date"
              aria-label="To date"
              value={search.to ?? ""}
              min={search.from}
              onChange={(e) => set({ to: e.currentTarget.value || undefined })}
              className={s.date}
            />
          </label>
          {filtered ? (
            <Button
              variant="ghost"
              size="sm"
              icon={<X size={14} />}
              onClick={() => void navigate({ search: {}, replace: true })}
            >
              Clear
            </Button>
          ) : null}
        </div>
      </div>

      {log.isPending ? (
        <SkeletonRows rows={10} />
      ) : log.isError ? (
        <ErrorState error={log.error} retry={() => void log.refetch()} />
      ) : entries.length === 0 ? (
        filtered ? (
          <Empty icon="search" title="No entries match">
            Try a wider date range or fewer filters.
          </Empty>
        ) : (
          <Empty icon="history" title="Nothing recorded yet">
            Cases, actions, DMs, appeals, rule changes, and setting changes in this server are
            recorded here.
          </Empty>
        )
      ) : (
        <div className={s.results}>
          <ol className={cx(s.list, log.isRefetching && s.dim)}>
            {entries.map((e) => (
              <ModLogEntry key={e.id} guildId={guildId} entry={e} />
            ))}
          </ol>
          {log.hasNextPage ? (
            <div className={s.more}>
              <Button
                variant="secondary"
                pending={log.isFetchingNextPage}
                onClick={() => void log.fetchNextPage()}
              >
                Load older entries
              </Button>
            </div>
          ) : entries.length > 20 ? (
            <p className={s.end}>That's the oldest entry.</p>
          ) : null}
        </div>
      )}
    </Page>
  );
}

function id(v: unknown): string | undefined {
  return typeof v === "string" && /^\d{15,21}$/.test(v) ? v : undefined;
}

function day(v: unknown): string | undefined {
  return typeof v === "string" && /^\d{4}-\d{2}-\d{2}$/.test(v) ? v : undefined;
}

function caseNumber(v: unknown): number | undefined {
  const n = typeof v === "number" ? v : typeof v === "string" ? Number(v) : Number.NaN;
  return Number.isSafeInteger(n) && n > 0 ? n : undefined;
}
