import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute, Link } from "@tanstack/react-router";
import { Radar } from "lucide-react";
import { useState } from "react";

import { ApiError, api, write } from "~/api/client";
import { guildMeQuery, healthQuery, honeypotModuleQuery, keys } from "~/api/queries";
import type { HoneypotModule } from "~/api/types";
import { ChannelPicker } from "~/features/channels/ChannelPicker";
import { MessageMarkdown, QuackMessage } from "~/features/discord/QuackMessage";
import { errorText } from "~/features/modules/enable";
import { honeypotCount, honeypotWarning, honeypotWarningOff } from "~/features/modules/honeypot";
import { ModuleHeader, StatTiles, useModuleSwitch } from "~/features/modules/ModuleParts";
import { useCan } from "~/lib/permissions";
import { Field, Select, TextArea } from "~/ui/Field";
import { Heading, Page, Section } from "~/ui/Page";
import { SaveBar } from "~/ui/SaveBar";
import { Empty, ErrorState, SkeletonRows } from "~/ui/States";
import { toast } from "~/ui/Toast";

import s from "./honeypot.module.css";

export const Route = createFileRoute("/_authed/guilds/$guildId/modules/honeypot")({
  loader: async ({ context, params }) => {
    const me = await context.queryClient.ensureQueryData(guildMeQuery(params.guildId));
    if (me.capabilities.manage_settings) {
      await context.queryClient.prefetchQuery(honeypotModuleQuery(params.guildId));
    }
  },
  component: HoneypotPage,
});

function HoneypotPage() {
  const { guildId } = Route.useParams();
  const can = useCan(guildId);
  return (
    <Page
      icon={<Radar size={22} />}
      title="Honeypot"
      topic="A trap channel for spam accounts."
      width="narrow"
    >
      {can("manage_settings") ? (
        <Honeypot guildId={guildId} />
      ) : (
        <Empty icon="lock" title="The honeypot needs Manage Server">
          To change Quack's settings, you need Manage Server in Discord. Ask an administrator to
          check your roles in Server Settings → Roles.
        </Empty>
      )}
    </Page>
  );
}

function Honeypot({ guildId }: { guildId: string }) {
  const honeypot = useQuery(honeypotModuleQuery(guildId));
  const toggle = useModuleSwitch(guildId, "honeypot");
  const [dirty, setDirty] = useState(false);

  return (
    <>
      <ModuleHeader
        module="honeypot"
        icon="shield"
        state={honeypot.data}
        onToggle={toggle.toggle}
        pending={toggle.pending}
        canManage
        locked={dirty ? "Save or reset your changes before turning the honeypot on or off." : null}
        problem={toggle.problem}
      >
        Choose a channel no real member would post in. When a member posts there, Quack adds a case
        under the honeypot's rule and deletes the message. Quack ignores posts from staff and bots.
      </ModuleHeader>

      {honeypot.isPending ? (
        <SkeletonRows rows={6} />
      ) : honeypot.isError ? (
        <ErrorState error={honeypot.error} retry={() => void honeypot.refetch()} />
      ) : (
        <>
          <Status data={honeypot.data} />
          <HoneypotForm guildId={guildId} data={honeypot.data} onDirtyChange={setDirty} />
        </>
      )}
    </>
  );
}

function Status({ data }: { data: HoneypotModule }) {
  return (
    <Section title="Activity">
      <StatTiles
        items={[
          {
            label: "Cases added",
            value: data.cases_created,
            hint: "Cases Quack added from posts in the honeypot channel.",
          },
          {
            label: "In progress",
            value: data.pending_hits,
            hint: "Posts Quack is still adding a case for.",
          },
          {
            label: "Failed",
            value: data.failed_hits,
            tone: data.failed_hits > 0 ? "danger" : undefined,
            hint: "Posts Quack couldn't add a case for. The moderation log says why.",
          },
        ]}
      />
    </Section>
  );
}

type Draft = { channel: string; rule: string; warning: string };

const draftFrom = (data: HoneypotModule): Draft => ({
  channel: data.settings.channel_id ?? "",
  rule: data.settings.rule_id ?? "",
  warning: data.settings.warning_text ?? "",
});

const differs = (a: Draft, b: Draft) =>
  a.channel !== b.channel || a.rule !== b.rule || a.warning.trim() !== b.warning.trim();

function HoneypotForm({
  guildId,
  data,
  onDirtyChange,
}: {
  guildId: string;
  data: HoneypotModule;
  onDirtyChange: (dirty: boolean) => void;
}) {
  const queryClient = useQueryClient();
  const can = useCan(guildId);
  const [edits, setEdits] = useState<Draft | null>(null);
  const [saveError, setSaveError] = useState<unknown>(null);
  const base = draftFrom(data);
  const form = edits ?? base;
  const dirty = differs(form, base);
  const enabled = data.enabled;

  // The preview words the warning from the chosen rule's levels, which
  // the honeypot page carries for every eligible rule; only the link to the
  // rule needs a rule reader.
  const canReadRules = can("moderate") || can("manage_rules");

  const set = (patch: Partial<Draft>) => {
    const next = { ...form, ...patch };
    setEdits(next);
    setSaveError(null);
    onDirtyChange(differs(next, base));
  };
  const reset = () => {
    setEdits(null);
    setSaveError(null);
    onDirtyChange(false);
  };

  const eligible = data.eligible_rules;
  const ruleUsable = !form.rule || eligible.some((r) => r.id === form.rule);
  const apiField = (path: string) =>
    saveError instanceof ApiError ? saveError.field(path) : undefined;
  const channelError =
    apiField("channel_id") ??
    (enabled && !form.channel ? "The honeypot needs a channel while it's on" : null);
  const ruleError =
    apiField("rule_id") ??
    (enabled && !form.rule
      ? "The honeypot needs a rule while it's on"
      : ruleUsable
        ? null
        : "The honeypot can't use this rule. It may be archived or have required case context.");
  const blocked = Boolean(
    (enabled && !form.channel) || (enabled && !form.rule) || (enabled && !ruleUsable),
  );

  const save = useMutation({
    mutationFn: () =>
      write((init) =>
        api.PUT("/api/guilds/{guild_id}/modules/honeypot/settings", {
          params: { path: { guild_id: guildId } },
          body: {
            expected_version: data.version,
            channel_id: form.channel || null,
            rule_id: form.rule || null,
            warning_text: form.warning.trim() || null,
          },
          ...init,
        }),
      ),
    onSuccess: (result) => {
      queryClient.setQueryData(honeypotModuleQuery(guildId).queryKey, result);
      void queryClient.invalidateQueries({ queryKey: keys.modules(guildId) });
      void queryClient.invalidateQueries({ queryKey: healthQuery(guildId).queryKey });
      reset();
      toast.success("Honeypot settings saved.");
    },
    onError: setSaveError,
  });

  const ruleDefinition = data.eligible_rules.find((r) => r.id === form.rule);
  const warning = enabled ? honeypotWarning(form.warning, ruleDefinition) : honeypotWarningOff;
  const currentName = eligible.find((r) => r.id === form.rule)?.name;

  return (
    <>
      <Section
        title="Honeypot channel and rule"
        description="Where the honeypot is and which rule its cases use."
      >
        <div className={s.fields}>
          <Field
            label="Honeypot channel"
            error={channelError}
            hint="A text channel no real member should post in. Quack needs View Channel, Send Messages, Read Message History, and Manage Messages there."
          >
            {(id) => (
              <ChannelPicker
                id={id}
                guildId={guildId}
                value={form.channel}
                onChange={(channel) => set({ channel })}
                types={["text"]}
                clearable={!enabled}
                noneLabel="No channel"
                invalid={Boolean(channelError)}
              />
            )}
          </Field>
          <Field
            label="Rule"
            error={ruleError}
            hint={
              form.rule && canReadRules ? (
                <>
                  Cases follow this rule's levels, DM, and appeal settings, like any other case.{" "}
                  <Link
                    to="/guilds/$guildId/rules/$ruleId"
                    params={{ guildId, ruleId: form.rule }}
                    className={s.link}
                  >
                    Open the rule
                  </Link>
                </>
              ) : (
                "Cases follow this rule's levels, DM, and appeal settings, like any other case. Only active rules without required case context can be used."
              )
            }
          >
            {(id) => (
              <Select
                id={id}
                value={form.rule}
                invalid={Boolean(ruleError)}
                onChange={(e) => set({ rule: e.currentTarget.value })}
              >
                <option value="" disabled={enabled}>
                  {enabled ? "Choose a rule" : "No rule"}
                </option>
                {form.rule && !currentName ? (
                  <option value={form.rule}>The current rule (can't be used)</option>
                ) : null}
                {eligible.map((r) => (
                  <option key={r.id} value={r.id}>
                    {r.name}
                  </option>
                ))}
              </Select>
            )}
          </Field>
        </div>
      </Section>

      <hr className={s.divider} />

      <Section
        title="Warning post"
        description="Quack posts this in the honeypot channel so real members know to stay out."
      >
        <Field
          label="Warning"
          hint="Leave it empty and Quack words it from the rule's levels. Quack adds the number of honeypot cases below it, and updates the post when you save."
        >
          {(id) => (
            <TextArea
              id={id}
              rows={4}
              value={form.warning}
              maxLength={1500}
              placeholder={honeypotWarning("", ruleDefinition) ?? "### Don't post here"}
              onChange={(e) => set({ warning: e.currentTarget.value })}
            />
          )}
        </Field>
        <div className={s.preview}>
          <Heading>{enabled ? "Preview" : "Preview (while the honeypot is off)"}</Heading>
          {warning ? (
            <QuackMessage label="Preview of the warning post">
              <MessageMarkdown
                text={enabled ? `${warning}\n-# ${honeypotCount(data.cases_created)}` : warning}
              />
            </QuackMessage>
          ) : (
            <p className={s.muted}>
              {form.rule
                ? "Quack words the warning from the rule's levels."
                : "Choose a rule to see the warning Quack will post."}
            </p>
          )}
        </div>
      </Section>

      <SaveBar
        visible={dirty}
        onReset={reset}
        onSave={() => save.mutate()}
        pending={save.isPending}
        error={errorText(saveError)}
        saveDisabled={blocked}
      />
    </>
  );
}
