import { useQuery } from "@tanstack/react-query";
import { History } from "lucide-react";
import { useMemo, useState } from "react";

import { ruleVersionQuery } from "~/api/queries";
import type { Rule, RuleDefinition } from "~/api/types";
import { UserChip } from "~/features/people/User";
import { ago, fullDate, plural } from "~/lib/format";
import { Badge } from "~/ui/Badge";
import { Button } from "~/ui/Button";
import { Dialog } from "~/ui/Dialog";
import { Panel, Section } from "~/ui/Page";
import { ErrorState, SkeletonRows } from "~/ui/States";

import { fieldTypes } from "./FieldsEditor";
import { fromDefinition, ladder, windowLabel } from "./draft";
import { Ladder } from "./Ladder";
import s from "./VersionHistory.module.css";

/**
 * VersionHistory lists a rule's saved versions, newest first. Each case
 * keeps the version it used, so opening an old version shows exactly what
 * those cases were added under. Versions never change once saved.
 */
export function VersionHistory({ guildId, rule }: { guildId: string; rule: Rule }) {
  const [open, setOpen] = useState<number | null>(null);
  const versions = useMemo(
    () => [...rule.versions].sort((a, b) => b.version - a.version),
    [rule.versions],
  );
  return (
    <Section
      title="History"
      description="Every saved version of this rule. Cases keep the version they were added under."
    >
      <Panel padded={false}>
        <ul className={s.list}>
          {versions.map((v) => (
            <li key={v.version} className={s.row}>
              <span className={s.version}>Version {v.version}</span>
              {v.version === rule.current_version ? <Badge tone="brand">Current</Badge> : null}
              {v.reconstructed ? <Badge tone="neutral">Rebuilt from old cases</Badge> : null}
              <span className={s.meta}>
                <span title={fullDate(v.created_at)}>Saved {ago(v.created_at)}</span>
                {v.created_by ? (
                  <>
                    {" by "}
                    <UserChip guildId={guildId} userId={v.created_by} size={18} />
                  </>
                ) : null}
              </span>
              <Button
                size="sm"
                variant="ghost"
                icon={<History size={16} />}
                onClick={() => setOpen(v.version)}
              >
                View
              </Button>
            </li>
          ))}
        </ul>
      </Panel>
      <VersionDialog
        guildId={guildId}
        ruleId={rule.id}
        version={open}
        onClose={() => setOpen(null)}
      />
    </Section>
  );
}

function VersionDialog({
  guildId,
  ruleId,
  version,
  onClose,
}: {
  guildId: string;
  ruleId: string;
  version: number | null;
  onClose: () => void;
}) {
  const query = useQuery({
    ...ruleVersionQuery(guildId, ruleId, version ?? 0),
    enabled: version !== null,
  });
  const data = query.data;
  return (
    <Dialog
      open={version !== null}
      onClose={onClose}
      size="md"
      title={`Version ${version ?? ""}`}
      description={
        data?.reconstructed
          ? "Quack rebuilt this version from cases imported from an earlier version of Quack, so some details may be missing."
          : data
            ? `Saved ${fullDate(data.created_at)}.`
            : undefined
      }
      footer={
        <Button variant="secondary" onClick={onClose}>
          Close
        </Button>
      }
    >
      {query.isPending ? (
        <SkeletonRows rows={3} />
      ) : query.isError ? (
        <ErrorState error={query.error} retry={() => void query.refetch()} />
      ) : data?.definition ? (
        <DefinitionView definition={data.definition} />
      ) : (
        <p className={s.meta}>
          Quack doesn't have the details of this version. Cases added under it still show their own
          reason and case context.
        </p>
      )}
    </Dialog>
  );
}

/** DefinitionView shows a rule definition read-only. */
export function DefinitionView({ definition }: { definition: RuleDefinition }) {
  const steps = useMemo(() => ladder(fromDefinition(definition).levels), [definition]);
  const kindLabel = (kind: string) => fieldTypes.find((t) => t.value === kind)?.label ?? kind;
  return (
    <div className={s.view}>
      <p className={s.name}>{definition.name}</p>
      {definition.description ? <p className={s.meta}>{definition.description}</p> : null}
      <div className={s.badges}>
        <Badge tone={definition.appealable ? "success" : "neutral"} dot>
          {definition.appealable ? "Appealable" : "No appeals"}
        </Badge>
        <Badge tone="neutral">{windowLabel(definition.counting_window_days)}</Badge>
      </div>
      <p className={s.reason}>
        <span className={s.label}>Reason: </span>
        {definition.reason}
      </p>
      <div className={s.ladder}>
        <Ladder steps={steps} />
      </div>
      {definition.fields.length > 0 ? (
        <div>
          <p className={s.label}>{plural(definition.fields.length, "case context field")}</p>
          <ul className={s.fields}>
            {definition.fields.map((f) => (
              <li key={f.key}>
                {f.label} <span className={s.meta}>· {kindLabel(f.kind)}</span>
                {f.required ? <span className={s.meta}> · required</span> : null}
              </li>
            ))}
          </ul>
        </div>
      ) : null}
    </div>
  );
}
