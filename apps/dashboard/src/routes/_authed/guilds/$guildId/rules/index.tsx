import { useQuery, useSuspenseQuery } from "@tanstack/react-query";
import { createFileRoute, Link } from "@tanstack/react-router";
import { BookOpen, ChevronRight, FileDown, Plus } from "lucide-react";
import { useMemo, useState } from "react";

import { rulesQuery } from "~/api/queries";
import type { RuleSummary } from "~/api/types";
import { ImportDialog } from "~/features/rules/ImportDialog";
import { StarterNotice } from "~/features/rules/StarterNotice";
import { ago, capitalize, fullDate, plural } from "~/lib/format";
import { cx } from "~/lib/cx";
import { useCan } from "~/lib/permissions";
import { Badge } from "~/ui/Badge";
import { Button, ButtonLink } from "~/ui/Button";
import { Page } from "~/ui/Page";
import { Empty, ErrorState, SkeletonRows } from "~/ui/States";

import s from "./rules.module.css";

export const Route = createFileRoute("/_authed/guilds/$guildId/rules/")({
  loader: ({ context, params }) => context.queryClient.ensureQueryData(rulesQuery(params.guildId)),
  component: Rules,
  errorComponent: ({ error, reset }) => (
    <Page icon={<BookOpen size={22} />} title="Rules">
      <ErrorState error={error} retry={reset} />
    </Page>
  ),
});

function Rules() {
  const { guildId } = Route.useParams();
  const { data: rules } = useSuspenseQuery(rulesQuery(guildId));
  const can = useCan(guildId);
  const [importing, setImporting] = useState(false);
  const [showArchived, setShowArchived] = useState(false);
  // Archived rules load when someone opens the list.
  const archivedRules = useQuery({ ...rulesQuery(guildId, true), enabled: showArchived });

  const active = useMemo(() => [...rules].sort((a, b) => a.name.localeCompare(b.name)), [rules]);
  const archived = useMemo(
    () => [...(archivedRules.data ?? [])].sort((a, b) => a.name.localeCompare(b.name)),
    [archivedRules.data],
  );
  const write = can("manage_rules");

  return (
    <Page
      icon={<BookOpen size={22} />}
      title="Rules"
      topic="What moderators apply. Each rule sets the reason, the case context, and the action at each level."
      actions={
        write ? (
          <>
            <Button
              size="sm"
              variant="secondary"
              icon={<FileDown size={16} />}
              onClick={() => setImporting(true)}
            >
              Import
            </Button>
            <ButtonLink
              size="sm"
              icon={<Plus size={16} />}
              to="/guilds/$guildId/rules/new"
              params={{ guildId }}
            >
              New rule
            </ButtonLink>
          </>
        ) : null
      }
    >
      <StarterNotice guildId={guildId} />

      {active.length === 0 ? (
        <Empty
          icon="shield"
          title="No active rules"
          action={
            write ? (
              <ButtonLink to="/guilds/$guildId/rules/new" params={{ guildId }}>
                Create a rule
              </ButtonLink>
            ) : null
          }
        >
          Moderators add cases by applying a rule, so this server needs at least one. Name rules
          after the problem, like Spam or Harassment, not the action.
        </Empty>
      ) : (
        <ul className={s.list}>
          {active.map((t) => (
            <RuleCard key={t.id} guildId={guildId} rule={t} />
          ))}
        </ul>
      )}

      {
        <details
          open={showArchived}
          onToggle={(e) => setShowArchived(e.currentTarget.open)}
          className={s.archived}
        >
          <summary className={s.summary}>
            <ChevronRight size={16} className={s.chevron} />
            Archived rules
            <span className={s.summaryHint}>
              Moderators can't apply them. Their cases stay in the history and keep counting.
            </span>
          </summary>
          {archivedRules.isPending ? (
            <SkeletonRows rows={2} />
          ) : archivedRules.isError ? (
            <ErrorState error={archivedRules.error} retry={() => void archivedRules.refetch()} />
          ) : archived.length === 0 ? (
            <p className={s.summaryHint}>No archived rules.</p>
          ) : (
            <ul className={cx(s.list, s.archivedList)}>
              {archived.map((t) => (
                <RuleCard key={t.id} guildId={guildId} rule={t} />
              ))}
            </ul>
          )}
        </details>
      }

      {write ? (
        <ImportDialog
          guildId={guildId}
          open={importing}
          onClose={() => setImporting(false)}
          takenNames={rules.map((t) => t.name)}
        />
      ) : null}
    </Page>
  );
}

function RuleCard({ guildId, rule }: { guildId: string; rule: RuleSummary }) {
  return (
    <li>
      <Link
        to="/guilds/$guildId/rules/$ruleId"
        params={{ guildId, ruleId: rule.id }}
        data-archived={rule.archived_at ? true : undefined}
        className={s.card}
      >
        <div className={s.cardHead}>
          <div className={s.titles}>
            <p className={s.name}>{rule.name}</p>
            {rule.description ? <p className={s.description}>{rule.description}</p> : null}
          </div>
          <ChevronRight size={20} className={s.go} />
        </div>
        <p className={s.levels}>{capitalize(rule.levels_text)}</p>
        <div className={s.badges}>
          {rule.archived_at ? (
            <Badge tone="neutral" dot>
              <span title={fullDate(rule.archived_at)}>Archived {ago(rule.archived_at)}</span>
            </Badge>
          ) : null}
          {rule.is_starter ? <Badge tone="brand">Starter rule</Badge> : null}
          {rule.used_by_honeypot ? <Badge tone="neutral">Used by the honeypot</Badge> : null}
          {rule.field_count ? (
            <Badge tone="neutral">{plural(rule.field_count, "case context field")}</Badge>
          ) : null}
          <span className={s.version}>Version {rule.current_version}</span>
        </div>
      </Link>
    </li>
  );
}
