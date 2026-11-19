import { useMutation, useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import { Archive, BookOpen, FileUp } from "lucide-react";
import { useCallback, useMemo, useState } from "react";

import { ApiError } from "~/api/client";
import { keys, ruleQuery } from "~/api/queries";
import { archiveRule, downloadRuleFile, exportRule, restoreRule, updateRule } from "~/api/rules";
import type { Rule } from "~/api/types";
import { Banner } from "~/features/rules/Banner";
import { fromDefinition, toDefinition } from "~/features/rules/draft";
import { RuleEditor } from "~/features/rules/RuleEditor";
import { StarterNotice } from "~/features/rules/StarterNotice";
import { VersionHistory } from "~/features/rules/VersionHistory";
import { ago, fullDate } from "~/lib/format";
import { useCan } from "~/lib/permissions";
import { Button } from "~/ui/Button";
import { ConfirmDialog } from "~/ui/ConfirmDialog";
import { Page } from "~/ui/Page";
import { ErrorState, InlineError } from "~/ui/States";
import { toast } from "~/ui/Toast";

export const Route = createFileRoute("/_authed/guilds/$guildId/rules/$ruleId")({
  loader: ({ context, params }) =>
    context.queryClient.ensureQueryData(ruleQuery(params.guildId, params.ruleId)),
  component: RulePage,
  errorComponent: ({ error, reset }) => (
    <Page icon={<BookOpen size={22} />} title="Rule">
      <ErrorState error={error} retry={reset} />
    </Page>
  ),
});

function RulePage() {
  const { guildId, ruleId } = Route.useParams();
  // A fresh page per rule, so moving between rules never carries a draft over.
  return <RuleView key={ruleId} guildId={guildId} ruleId={ruleId} />;
}

function RuleView({ guildId, ruleId }: { guildId: string; ruleId: string }) {
  const query = ruleQuery(guildId, ruleId);
  const { data: rule, refetch } = useSuspenseQuery(query);
  const can = useCan(guildId);
  const queryClient = useQueryClient();
  const [archiving, setArchiving] = useState(false);
  const [archiveError, setArchiveError] = useState<string | null>(null);
  const [restoreError, setRestoreError] = useState<string | null>(null);
  // The version the editor started from. Background refetches update rule
  // but not this, so they never wipe a draft or move expected_version past
  // someone else's save; the rules manager picks up a newer version on purpose.
  const [base, setBase] = useState({ rule, loads: 0 });
  const load = useCallback(
    (next: Rule) => setBase((b) => ({ rule: next, loads: b.loads + 1 })),
    [],
  );

  const archived = Boolean(rule.archived_at);
  const write = can("manage_rules");
  const initial = useMemo(() => fromDefinition(base.rule.definition), [base.rule]);
  const newer = rule.current_version > base.rule.current_version;
  const name = rule.definition.name;

  const settle = useCallback(
    (next: Rule) => {
      queryClient.setQueryData(query.queryKey, next);
      void queryClient.invalidateQueries({ queryKey: [...keys.rules(guildId), "list"] });
      load(next);
    },
    [guildId, load, query.queryKey, queryClient],
  );

  const archive = useMutation({
    mutationFn: () => archiveRule(guildId, ruleId),
    onSuccess: (next) => {
      settle(next);
      setArchiving(false);
      toast.success(`${next.definition.name} is archived. Its cases stay in the history.`);
    },
    onError: (e) =>
      setArchiveError(e instanceof ApiError ? e.message : "Quack couldn't archive the rule."),
  });
  const restore = useMutation({
    mutationFn: () => restoreRule(guildId, ruleId),
    onSuccess: (next) => {
      setRestoreError(null);
      settle(next);
      toast.success(`${next.definition.name} is restored. Moderators can apply it again.`);
    },
    onError: (e) =>
      setRestoreError(e instanceof ApiError ? e.message : "Quack couldn't restore the rule."),
  });
  const exporting = useMutation({
    mutationFn: () => exportRule(guildId, ruleId),
    onSuccess: (file) => downloadRuleFile(file),
  });

  const onSaved = useCallback(
    (next: Rule) => {
      settle(next);
      toast.success(`Saved version ${next.current_version}. New cases use it from now on.`);
    },
    [settle],
  );

  return (
    <Page
      icon={<BookOpen size={22} />}
      title={name}
      topic={`Version ${rule.current_version}${archived ? " · Archived" : ""}`}
      actions={
        <>
          {write ? (
            <Button
              size="sm"
              variant="secondary"
              icon={<FileUp size={16} />}
              pending={exporting.isPending}
              onClick={() => exporting.mutate()}
            >
              Export rule file
            </Button>
          ) : null}
          {write && !archived ? (
            <Button
              size="sm"
              variant="danger"
              icon={<Archive size={16} />}
              onClick={() => setArchiving(true)}
            >
              Archive
            </Button>
          ) : null}
        </>
      }
    >
      <RuleEditor
        key={base.loads}
        initial={initial}
        readOnly={!write || archived}
        onSave={(draft) =>
          updateRule(guildId, ruleId, base.rule.current_version, toDefinition(draft))
        }
        onSaved={onSaved}
        onReload={() =>
          void refetch().then((r) => {
            if (r.data) load(r.data);
          })
        }
      >
        {newer && !archived ? (
          <Banner
            tone="warning"
            icon="history"
            title={`Someone saved version ${rule.current_version}`}
            action={
              <Button size="sm" variant="secondary" onClick={() => load(rule)}>
                Load it
              </Button>
            }
          >
            You're looking at version {base.rule.current_version}. Load the new one before editing.
            Loading it discards any changes you made here.
          </Banner>
        ) : null}
        {archived ? (
          <Banner
            tone="neutral"
            icon="lock"
            title={<span title={fullDate(rule.archived_at)}>Archived {ago(rule.archived_at)}</span>}
            action={
              write ? (
                <Button
                  size="sm"
                  variant="success"
                  pending={restore.isPending}
                  onClick={() => restore.mutate()}
                >
                  Restore
                </Button>
              ) : null
            }
          >
            Moderators can't apply this rule. Its cases stay in the history, keep counting, and
            still show which version they used. Restore it to edit or apply it again.
            {restoreError ? <InlineError>{restoreError}</InlineError> : null}
          </Banner>
        ) : !write ? (
          <Banner tone="neutral" icon="lock" title="View only">
            To edit rules, you need Manage Server in Discord or a rules manager role in Quack.
          </Banner>
        ) : (
          <StarterNotice guildId={guildId} ruleId={ruleId} />
        )}
      </RuleEditor>

      <VersionHistory guildId={guildId} rule={rule} />

      <ConfirmDialog
        open={archiving}
        onClose={() => {
          setArchiving(false);
          setArchiveError(null);
        }}
        title={`Archive ${name}?`}
        description="Moderators won't be able to apply it anymore. Its cases stay in the history and keep counting, and you can restore it any time."
        confirmLabel="Archive rule"
        tone="danger"
        pending={archive.isPending}
        error={archiveError}
        onConfirm={() => archive.mutate()}
      />
    </Page>
  );
}
