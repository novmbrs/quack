import { useMutation, useQuery, useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { createFileRoute, Link, redirect } from "@tanstack/react-router";
import { ChevronRight, LayoutGrid } from "lucide-react";
import type { ReactNode } from "react";

import {
  casesQuery,
  guildMeQuery,
  healthQuery,
  settingsQuery,
  statisticsQuery,
} from "~/api/queries";
import { acknowledgeStarterNotice } from "~/api/rules";
import type { ActionKind, Bucket, CaseSummary, Health } from "~/api/types";
import { ActivityChart } from "~/features/overview/ActivityChart";
import { fillDays, total } from "~/features/overview/days";
import { showsOverview } from "~/features/shell/guilds";
import { UserChip } from "~/features/people/User";
import { actionMeta, actionOutcome, ago, plural } from "~/lib/format";
import { useCan } from "~/lib/permissions";
import { Badge } from "~/ui/Badge";
import { Button, ButtonLink } from "~/ui/Button";
import { Heading, Page, Panel } from "~/ui/Page";
import { QuackIcon, type QuackIconName } from "~/ui/QuackIcon";
import { Empty, Skeleton } from "~/ui/States";
import { cx } from "~/lib/cx";

import s from "./overview.module.css";

const DAYS = 30;

/** statsWindow covers the last 30 UTC days, stable for the whole day. */
function statsWindow(now = new Date()) {
  const tomorrow = Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate() + 1);
  const from = new Date(tomorrow - DAYS * 86400_000);
  return { from, fromIso: from.toISOString(), toIso: new Date(tomorrow).toISOString() };
}

export const Route = createFileRoute("/_authed/guilds/$guildId/")({
  loader: async ({ context, params }) => {
    // The layout loads and reports /me itself; this only needs it to choose.
    const me = await context.queryClient
      .ensureQueryData(guildMeQuery(params.guildId))
      .catch(() => null);
    if (!me) return;
    // Rules managers have nothing here, so their server opens on Rules.
    if (!showsOverview(me)) throw redirect({ to: "/guilds/$guildId/rules", params });
    const { fromIso, toIso } = statsWindow();
    // Start every panel's data together; the page renders as each arrives.
    if (me.capabilities.moderate) {
      void context.queryClient.prefetchQuery(statisticsQuery(params.guildId, fromIso, toIso));
      void context.queryClient.prefetchQuery(casesQuery(params.guildId, { limit: 6 }));
    }
    if (me.capabilities.manage_settings)
      void context.queryClient.prefetchQuery(healthQuery(params.guildId));
  },
  component: Overview,
});

function Overview() {
  const { guildId } = Route.useParams();
  const { data: me } = useSuspenseQuery(guildMeQuery(guildId));
  const can = useCan(guildId);
  const range = statsWindow();

  const moderates = can("moderate");
  const manages = can("manage_settings");
  const stats = useQuery({
    ...statisticsQuery(guildId, range.fromIso, range.toIso),
    enabled: moderates,
  });
  const recent = useQuery({ ...casesQuery(guildId, { limit: 6 }), enabled: moderates });
  const settings = useQuery({ ...settingsQuery(guildId), enabled: manages });
  const health = useQuery({ ...healthQuery(guildId), enabled: manages, retry: false });

  const st = stats.data;
  const voided = st?.cases_by_state.find((b) => b.key === "voided")?.count ?? 0;
  const starter =
    settings.data?.starter_rule_id && !settings.data.starter_notice_acknowledged_at
      ? settings.data.starter_rule_id
      : null;

  return (
    <Page icon={<LayoutGrid size={22} />} title="Overview" topic={me.guild.name}>
      {starter ? <StarterReminder guildId={guildId} ruleId={starter} /> : null}

      <div className={s.tiles}>
        {moderates ? (
          <AttentionTile
            to="/guilds/$guildId/appeals"
            guildId={guildId}
            icon="appeal"
            label="Appeals waiting for a decision"
            value={me.counts?.pending_appeals}
            calm="No appeals waiting for a decision"
          />
        ) : null}
        {moderates ? (
          <AttentionTile
            to="/guilds/$guildId/failures"
            guildId={guildId}
            icon="error"
            label="Failed actions"
            value={me.counts?.failed_actions}
            calm="No failed actions"
          />
        ) : null}
        {manages && health.data ? <HealthTile guildId={guildId} health={health.data} /> : null}
      </div>

      {moderates ? (
        <section className={s.section}>
          <Heading>Last 30 days</Heading>
          <div className={s.stats}>
            <Stat label="Cases" value={st?.cases_total} />
            <Stat label="Actions" value={st ? total(st.actions_by_kind) : undefined} />
            <Stat label="Appeals" value={st ? total(st.appeals_by_state) : undefined} />
            <Stat label="Voided" value={st ? voided : undefined} />
          </div>
          <Panel>
            <p className={s.chartTitle}>Cases per day</p>
            {st ? (
              <ActivityChart data={fillDays(st.cases_by_day, range.from, DAYS)} />
            ) : (
              <Skeleton height={160} />
            )}
          </Panel>
          <div className={s.split}>
            <Panel>
              <p className={s.chartTitle}>Most used rules</p>
              <Breakdown buckets={st?.cases_by_rule} empty="No cases in the last 30 days." />
            </Panel>
            <Panel>
              <p className={s.chartTitle}>Actions</p>
              <Breakdown
                buckets={st?.actions_by_kind}
                icon={(key) => actionMeta[key as ActionKind]?.icon}
                empty="No timeouts, kicks, or bans."
              />
            </Panel>
          </div>
        </section>
      ) : null}

      {moderates ? (
        <section className={s.section}>
          <Heading
            actions={
              <Link to="/guilds/$guildId/cases" params={{ guildId }} className={s.more}>
                All cases <ChevronRight size={14} />
              </Link>
            }
          >
            Recent cases
          </Heading>
          <Panel padded={false}>
            {recent.isPending ? (
              <div className={s.recentPad}>
                <Skeleton height={120} />
              </div>
            ) : recent.data?.cases.length ? (
              <ul className={s.recent}>
                {recent.data.cases.map((c) => (
                  <li key={c.id}>
                    <Link
                      to="/guilds/$guildId/cases/$caseRef"
                      params={{ guildId, caseRef: String(c.number) }}
                      className={s.recentRow}
                    >
                      <span className={s.caseNo}>#{c.number}</span>
                      <UserChip guildId={guildId} userId={c.member_id} link={false} />
                      <span className={s.ruleName}>{c.rule_name}</span>
                      <CaseAction c={c} />
                      <span className={s.when}>{ago(c.created_at)}</span>
                    </Link>
                  </li>
                ))}
              </ul>
            ) : (
              <Empty icon="case" title="No cases yet" compact>
                Use /case add in Discord, or add one from the Cases page.
              </Empty>
            )}
          </Panel>
        </section>
      ) : null}
    </Page>
  );
}

/**
 * CaseAction is a recent case's action and its outcome word, or "Warning"
 * for a case with no action.
 */
function CaseAction({ c }: { c: CaseSummary }) {
  if (c.voided_at) return <Badge tone="neutral">Voided</Badge>;
  if (!c.action) return <Badge tone="neutral">Warning</Badge>;
  const outcome = actionOutcome(c.action);
  return (
    <Badge tone={outcome.tone} dot title={c.action.outcome_text}>
      {actionMeta[c.action.kind].label} · {outcome.label}
    </Badge>
  );
}

/**
 * StarterReminder asks administrators to review the rule Quack created when
 * it joined, until one of them dismisses it.
 */
function StarterReminder({ guildId, ruleId }: { guildId: string; ruleId: string }) {
  const queryClient = useQueryClient();
  const dismiss = useMutation({
    mutationFn: () => acknowledgeStarterNotice(guildId),
    onSuccess: (settings) => queryClient.setQueryData(settingsQuery(guildId).queryKey, settings),
  });
  return (
    <Banner
      icon="spark"
      title="Review your starter rule"
      action={
        <div className={s.bannerActions}>
          <ButtonLink size="sm" to="/guilds/$guildId/rules/$ruleId" params={{ guildId, ruleId }}>
            Review rule
          </ButtonLink>
          <Button
            size="sm"
            variant="ghost"
            pending={dismiss.isPending}
            onClick={() => dismiss.mutate()}
          >
            Dismiss
          </Button>
        </div>
      }
    >
      Quack added a General rule violation rule when it joined: a warning, then a timeout for 1 day
      at 3 cases, then a ban at 5 cases. Make sure it fits your server.
    </Banner>
  );
}

/**
 * HealthTile sums up server health (GET …/health) for administrators: what
 * Quack is missing, with the details on the settings page.
 */
function HealthTile({ guildId, health }: { guildId: string; health: Health }) {
  const problems = healthProblems(health);
  const ok = problems.length === 0;
  return (
    <Link
      to="/guilds/$guildId/settings"
      params={{ guildId }}
      className={cx(s.tile, s.tileLink, !ok && s.tileHot)}
    >
      <QuackIcon name={ok ? "shield" : "warn"} size={32} />
      <div className={s.tileText}>
        <span className={s.tileValueSmall}>{ok ? "All good" : "Needs a fix"}</span>
        <span className={s.tileLabel}>
          {ok ? "Quack has the permissions and channels it needs." : problems[0]}
        </span>
      </div>
      <ChevronRight size={18} className={s.tileChevron} />
    </Link>
  );
}

/** healthProblems lists what Quack needs fixed in a server, most important first. */
function healthProblems(health: Health): string[] {
  const out: string[] = [];
  if (!health.quack_in_server) out.push("Quack isn't in this server anymore.");
  if (health.missing_permissions.length > 0)
    out.push(
      `Quack is missing ${plural(health.missing_permissions.length, "Discord permission")}.`,
    );
  for (const channel of health.channels)
    for (const problem of channel.problems) out.push(problem.message);
  for (const module of Object.values(health.modules))
    for (const problem of module.problems) out.push(problem.message);
  return out;
}

function Banner({
  icon,
  title,
  children,
  action,
}: {
  icon: QuackIconName;
  title: string;
  children: ReactNode;
  action?: ReactNode;
}) {
  return (
    <div className={s.banner}>
      <QuackIcon name={icon} size={28} />
      <div className={s.bannerText}>
        <p className={s.bannerTitle}>{title}</p>
        <p className={s.bannerBody}>{children}</p>
      </div>
      {action}
    </div>
  );
}

function AttentionTile({
  to,
  guildId,
  icon,
  label,
  value,
  calm,
}: {
  to: "/guilds/$guildId/appeals" | "/guilds/$guildId/failures";
  guildId: string;
  icon: QuackIconName;
  label: string;
  value: number | undefined;
  calm: string;
}) {
  const hot = (value ?? 0) > 0;
  return (
    <Link to={to} params={{ guildId }} className={cx(s.tile, s.tileLink, hot && s.tileHot)}>
      <QuackIcon name={hot ? icon : "success"} size={32} />
      <div className={s.tileText}>
        {value === undefined ? (
          <Skeleton width={40} height={24} />
        ) : (
          <span className={s.tileValue}>{hot ? value.toLocaleString() : "0"}</span>
        )}
        <span className={s.tileLabel}>{hot ? label : calm}</span>
      </div>
      <ChevronRight size={18} className={s.tileChevron} />
    </Link>
  );
}

function Stat({ label, value }: { label: string; value: number | undefined }) {
  return (
    <Panel className={s.stat}>
      <span className={s.statLabel}>{label}</span>
      {value === undefined ? (
        <Skeleton width={56} height={28} />
      ) : (
        <span className={s.statValue}>{value.toLocaleString()}</span>
      )}
    </Panel>
  );
}

function Breakdown({
  buckets,
  icon,
  empty,
}: {
  buckets: Bucket[] | undefined;
  icon?: (key: string) => QuackIconName | undefined;
  empty: string;
}) {
  if (!buckets) return <Skeleton height={100} />;
  const rows = [...buckets].sort((a, b) => b.count - a.count).slice(0, 5);
  if (rows.length === 0) return <p className={s.emptyText}>{empty}</p>;
  const max = rows[0]!.count;
  return (
    <ul className={s.breakdown}>
      {rows.map((b) => {
        const name = icon?.(b.key);
        return (
          <li key={b.key} className={s.breakRow}>
            <div className={s.breakHead}>
              {name ? <QuackIcon name={name} size={16} /> : null}
              <span className={s.breakLabel}>{b.label}</span>
              <span className={s.breakValue} title={plural(b.count, "time")}>
                {b.count.toLocaleString()}
              </span>
            </div>
            <div className={s.track}>
              <div className={s.fill} style={{ width: `${(b.count / max) * 100}%` }} />
            </div>
          </li>
        );
      })}
    </ul>
  );
}
