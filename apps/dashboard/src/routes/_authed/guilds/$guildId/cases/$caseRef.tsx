import { useMutation, useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { createFileRoute, Link } from "@tanstack/react-router";
import { FolderOpen } from "lucide-react";
import { useState } from "react";

import { guildMeQuery, caseQuery, keys } from "~/api/queries";
import type { Case, Field } from "~/api/types";
import { CaseActions } from "~/features/cases/Actions";
import { EditContextDialog } from "~/features/cases/CaseEdits";
import { contextFields } from "~/features/cases/context";
import { CreateCaseDialog } from "~/features/cases/CreateCaseDialog";
import { EvidenceSection } from "~/features/cases/EvidenceSection";
import { Outcome, OutcomeBadge } from "~/features/cases/Outcome";
import { Timeline } from "~/features/cases/Timeline";
import { voidCase } from "~/features/cases/writes";
import { UserChip } from "~/features/people/User";
import {
  appealMeta,
  capitalize,
  countedCase,
  dmOutcome,
  fullDate,
  levelName,
  plural,
  sourceLabel,
  stamp,
} from "~/lib/format";
import { useCan } from "~/lib/permissions";
import { Badge } from "~/ui/Badge";
import { Button } from "~/ui/Button";
import { ConfirmDialog } from "~/ui/ConfirmDialog";
import { Details } from "~/ui/Details";
import { Heading, Page, Panel } from "~/ui/Page";
import { QuackIcon } from "~/ui/QuackIcon";
import { ErrorState, errorMessage } from "~/ui/States";
import { toast } from "~/ui/Toast";

import s from "./case.module.css";

export const Route = createFileRoute("/_authed/guilds/$guildId/cases/$caseRef")({
  loader: ({ context, params }) =>
    context.queryClient.ensureQueryData(caseQuery(params.guildId, params.caseRef)),
  component: CasePage,
  errorComponent: ({ error, reset }) => (
    <Page icon={<FolderOpen size={22} />} title="Case">
      <ErrorState error={error} retry={reset} />
    </Page>
  ),
});

/** Imported is what Quack v4 recorded for a case brought over from it. */
function CasePage() {
  const { guildId, caseRef } = Route.useParams();
  const { data: c } = useSuspenseQuery(caseQuery(guildId, caseRef));
  const can = useCan(guildId);
  const queryClient = useQueryClient();
  const [voiding, setVoiding] = useState(false);
  const [replacing, setReplacing] = useState(false);
  const [editing, setEditing] = useState(false);
  const [voidError, setVoidError] = useState<string | null>(null);

  const moderate = can("moderate");
  const voided = Boolean(c.voided_at);
  const imported = c.imported;
  const voidIt = useMutation({
    mutationFn: (reason: string) => voidCase(guildId, String(c.number), reason),
    onSuccess: (voided) => {
      setVoiding(false);
      toast.success(voided.message);
      void queryClient.invalidateQueries({ queryKey: keys.cases(guildId) });
      void queryClient.invalidateQueries({ queryKey: keys.appeals(guildId) });
      void queryClient.invalidateQueries({ queryKey: guildMeQuery(guildId).queryKey });
    },
    onError: (e) => setVoidError(errorMessage(e)),
  });

  const fields = contextFields(c.fields);
  const answered = fields.filter((f) => {
    const v = c.context[f.key];
    return v !== undefined && v !== "";
  });

  return (
    <Page
      icon={<FolderOpen size={22} />}
      title={`Case #${c.number}`}
      topic={c.rule_name}
      actions={
        moderate && !voided ? (
          <Button size="sm" variant="danger" onClick={() => setVoiding(true)}>
            Void case
          </Button>
        ) : null
      }
    >
      {c.void ? (
        <div className={s.voidBanner}>
          <QuackIcon name="case_void" size={24} />
          <div className={s.bannerText}>
            <p className={s.bannerTitle}>Case #{c.number} was voided. It no longer counts.</p>
            <p className={s.bannerBody}>
              {c.void.cause === "appeal_accepted" ? "The appeal was accepted. " : ""}
              {c.void.reason}{" "}
              {c.replaced_by_case_number ? (
                <Link
                  to="/guilds/$guildId/cases/$caseRef"
                  params={{ guildId, caseRef: String(c.replaced_by_case_number) }}
                  className={s.link}
                >
                  Replaced by case #{c.replaced_by_case_number}.
                </Link>
              ) : null}
            </p>
          </div>
          {moderate && !c.replaced_by_case_number && c.source !== "imported" ? (
            <Button size="sm" variant="secondary" onClick={() => setReplacing(true)}>
              Add replacement case
            </Button>
          ) : null}
        </div>
      ) : null}

      <div className={s.grid}>
        <div className={s.main}>
          <Panel className={s.hero}>
            <div className={s.heroTop}>
              <UserChip guildId={guildId} userId={c.member_id} size={48} subtitle />
              <div className={s.heroOutcome}>
                {voided ? (
                  <Badge tone="neutral" dot>
                    Voided
                  </Badge>
                ) : (
                  <Outcome action={c.action} />
                )}
              </div>
            </div>
            <div className={s.reason}>
              <span className={s.quoteBar} />
              <div>
                <p className={s.reasonLabel}>
                  {c.rule_name}
                  {c.level_index && c.level_from_case_count
                    ? ` · ${levelName(c.level_index, c.level_from_case_count)}`
                    : ""}
                </p>
                <p className={s.reasonText}>{c.reason}</p>
              </div>
            </div>
          </Panel>

          {imported ? (
            <p className={s.warn}>
              <QuackIcon name="info" size={16} /> Imported from Quack v4. The original action was{" "}
              {imported.v4_action || "unknown"}. Quack took no action for this case.
            </p>
          ) : null}

          <section className={s.block}>
            <div className={s.blockHead}>
              <Heading>Case context</Heading>
              {moderate ? (
                <Button size="sm" variant="ghost" onClick={() => setEditing(true)}>
                  Edit context
                </Button>
              ) : null}
            </div>
            {answered.length > 0 ? (
              <Panel>
                <Details
                  items={answered.map((f) => ({
                    label: f.label,
                    value: <ContextValueView field={f} value={c.context[f.key]!} />,
                  }))}
                />
              </Panel>
            ) : (
              <p className={s.muted}>No case context.</p>
            )}
          </section>

          <EvidenceSection
            // A new case gets a new section, so staged files, drafts, and
            // pending writes never follow the reader to another case.
            key={`${guildId}:${c.id}`}
            guildId={guildId}
            value={c}
            moderate={moderate}
            onCase={(updated) => {
              queryClient.setQueryData(caseQuery(guildId, caseRef).queryKey, updated);
              void queryClient.invalidateQueries({ queryKey: keys.cases(guildId) });
            }}
          />

          {c.timeline.length ? (
            <section className={s.block}>
              <Heading>History</Heading>
              <Panel>
                <Timeline
                  guildId={guildId}
                  items={c.timeline.map((e) => ({
                    at: e.created_at,
                    text: e.text,
                    actorId: e.actor_id ?? null,
                    result: e.result,
                    kind: e.kind,
                  }))}
                />
                {c.timeline_truncated ? (
                  <p className={s.muted}>
                    Only the latest entries are shown.{" "}
                    <Link
                      to="/guilds/$guildId/mod-log"
                      params={{ guildId }}
                      search={{ member: c.member_id }}
                      className={s.link}
                    >
                      Open the moderation log
                    </Link>{" "}
                    for the rest.
                  </p>
                ) : null}
              </Panel>
            </section>
          ) : null}
        </div>

        <aside className={s.side}>
          {c.source !== "imported" ? (
            <section className={s.block}>
              <Heading>Actions</Heading>
              <CaseActions guildId={guildId} memberId={c.member_id} actions={c.actions} />
            </section>
          ) : null}
          <Panel>
            <Details items={sideDetails(guildId, c)} />
          </Panel>
          <Link
            to="/guilds/$guildId/members/$userId"
            params={{ guildId, userId: c.member_id }}
            className={s.historyLink}
          >
            <QuackIcon name="history" size={18} /> Member's case history
          </Link>
        </aside>
      </div>

      <ConfirmDialog
        open={voiding}
        onClose={() => {
          setVoiding(false);
          setVoidError(null);
        }}
        title={`Void case #${c.number}?`}
        description="The case stays in the history but stops counting. If its timeout or ban is in place, Quack checks whether it can remove it. A kick can't be undone."
        confirmLabel="Void case"
        tone="danger"
        reason={{
          label: "Why are you voiding this case?",
          required: true,
          hint: "The member can see this reason.",
        }}
        pending={voidIt.isPending}
        error={voidError}
        onConfirm={(reason) => voidIt.mutate(reason)}
      />
      <CreateCaseDialog
        guildId={guildId}
        open={replacing}
        onClose={() => setReplacing(false)}
        initialMember={c.member_id}
        replacesCaseNumber={c.number}
      />
      {editing ? (
        <EditContextDialog
          guildId={guildId}
          value={c}
          open={editing}
          onClose={() => setEditing(false)}
        />
      ) : null}
    </Page>
  );
}

function sideDetails(guildId: string, c: Case) {
  const dm = c.dm;
  return [
    {
      label: "Moderator",
      value: <UserChip guildId={guildId} userId={c.moderator_id} size={20} />,
    },
    {
      label: "Added",
      value: <time title={fullDate(c.created_at)}>{stamp(c.created_at)}</time>,
    },
    { label: "From", value: sourceLabel[c.source] },
    c.case_count
      ? {
          label: "Case count",
          value: capitalize(countedCase(c.case_count)),
        }
      : null,
    c.source !== "imported"
      ? {
          label: "DM",
          value: dm ? (
            <span className={s.dm}>
              <OutcomeBadge
                outcome={dmOutcome[dm.state]}
                moving={dm.state === "pending" || dm.state === "sending"}
              />
              <span>{dm.outcome_text}</span>
            </span>
          ) : (
            "This level doesn't send a DM."
          ),
        }
      : null,
    c.rule_id
      ? {
          label: "Rule",
          value: (
            <Link
              to="/guilds/$guildId/rules/$ruleId"
              params={{ guildId, ruleId: c.rule_id }}
              className={s.link}
            >
              {c.rule_name}
              {c.rule_version ? ` (version ${c.rule_version})` : ""}
            </Link>
          ),
        }
      : null,
    c.replaces_case_number
      ? {
          label: "Replaces",
          value: (
            <Link
              to="/guilds/$guildId/cases/$caseRef"
              params={{ guildId, caseRef: String(c.replaces_case_number) }}
              className={s.link}
            >
              Case #{c.replaces_case_number}
            </Link>
          ),
        }
      : null,
    c.appeal
      ? {
          label: "Appeal",
          value: (
            <Link
              to="/guilds/$guildId/appeals/$appealId"
              params={{ guildId, appealId: c.appeal.id }}
              className={s.link}
            >
              {appealMeta[c.appeal.state].label}
            </Link>
          ),
        }
      : { label: "Appeal", value: c.appealable ? "Not appealed" : "Can't be appealed" },
    c.void
      ? {
          label: "Voided",
          value: (
            <span>
              {c.void.by ? <UserChip guildId={guildId} userId={c.void.by} size={20} /> : null}{" "}
              <time title={fullDate(c.void.at)}>{stamp(c.void.at)}</time>
            </span>
          ),
        }
      : null,
    c.evidence.some((e) => e.state === "failed" && !e.removed)
      ? {
          label: "Evidence",
          value: `Quack couldn't save ${plural(
            c.evidence.filter((e) => e.state === "failed" && !e.removed).length,
            "evidence item",
          )}. See why under Evidence.`,
        }
      : null,
  ];
}

function ContextValueView({ field, value }: { field: Field; value: string | boolean }) {
  if (typeof value === "boolean") return <>{value ? "Yes" : "No"}</>;
  if (field.kind === "message_link") {
    return (
      <a href={value} target="_blank" rel="noreferrer" className={s.link}>
        {value}
      </a>
    );
  }
  return <span className={s.pre}>{value}</span>;
}
