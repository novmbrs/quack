import { useMutation, useQuery, useQueryClient, useSuspenseQuery } from "@tanstack/react-query";
import { createFileRoute, Link } from "@tanstack/react-router";
import { Settings as SettingsIcon } from "lucide-react";
import { type ReactNode, useState } from "react";

import { ApiError, api, write } from "~/api/client";
import { rolesQuery } from "~/api/directory";
import { guildMeQuery, healthQuery, keys, settingsQuery } from "~/api/queries";
import { ChannelPicker } from "~/features/channels/ChannelPicker";
import { Callout, Command } from "~/features/modules/ModuleParts";
import { RolePicker } from "~/features/roles/RolePicker";
import { StarterNotice } from "~/features/rules/StarterNotice";
import { buildCaseDm, type DmAction } from "~/features/settings/caseDm";
import { DmPreview } from "~/features/settings/DmPreview";
import {
  changes,
  formFromSettings,
  inviteProblem,
  limits,
  maxStaffRoles,
  type SettingsForm,
  settingsFields,
  settingsPatch,
  staffRolesProblem,
  textProblem,
} from "~/features/settings/form";
import { healthChecklist, healthy } from "~/features/settings/health";
import { useCan } from "~/lib/permissions";
import { Badge } from "~/ui/Badge";
import { Button } from "~/ui/Button";
import { Field, TextArea, TextInput } from "~/ui/Field";
import { Heading, Page, Section } from "~/ui/Page";
import { QuackIcon } from "~/ui/QuackIcon";
import { SaveBar } from "~/ui/SaveBar";
import { Segmented } from "~/ui/Segmented";
import { ErrorState, SkeletonRows } from "~/ui/States";
import { toast } from "~/ui/Toast";

import s from "./settings.module.css";

export const Route = createFileRoute("/_authed/guilds/$guildId/settings")({
  loader: ({ context, params }) =>
    context.queryClient.ensureQueryData(settingsQuery(params.guildId)),
  component: SettingsPage,
  errorComponent: ({ error, reset }) => (
    <Page icon={<SettingsIcon size={22} />} title="Settings">
      <ErrorState error={error} retry={reset} />
    </Page>
  ),
});

type FieldErrors = Partial<Record<keyof SettingsForm, string>>;

function SettingsPage() {
  const { guildId } = Route.useParams();
  const { data: settings, refetch } = useSuspenseQuery(settingsQuery(guildId));
  const { data: me } = useSuspenseQuery(guildMeQuery(guildId));
  const can = useCan(guildId);
  const canWrite = can("manage_settings");
  const canWriteModerators = canWrite && can("manage_staff_roles");
  const queryClient = useQueryClient();

  const [edits, setEdits] = useState<SettingsForm | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [serverErrors, setServerErrors] = useState<FieldErrors>({});
  const [conflict, setConflict] = useState<string | null>(null);
  const saved = formFromSettings(settings);
  const form = edits ?? saved;
  const patch = settingsPatch(saved, form, settings.version);
  const dirty = changes(patch);

  const set = (change: Partial<SettingsForm>) => {
    setEdits({ ...form, ...change });
    setSaveError(null);
    setServerErrors((e) => {
      const next = { ...e };
      for (const k of Object.keys(change)) delete next[k as keyof SettingsForm];
      return next;
    });
  };
  const reset = () => {
    setEdits(null);
    setSaveError(null);
    setServerErrors({});
  };

  const inviteError = serverErrors.appealInvite ?? inviteProblem(form.appealInvite);
  const introError = serverErrors.dmIntro ?? textProblem(form.dmIntro, limits.dmIntro);
  const footerError = serverErrors.dmFooter ?? textProblem(form.dmFooter, limits.dmFooter);
  // Only a changed role list is checked on save, so only flag those.
  const roles = useQuery(rolesQuery(guildId));
  const knownRoles = roles.data ? new Set(roles.data.map((r) => r.id)) : undefined;
  const moderatorError =
    serverErrors.moderatorRoles ??
    (patch.moderator_role_ids
      ? staffRolesProblem(form.moderatorRoles, saved.moderatorRoles, knownRoles)
      : null);
  const rulesManagerError =
    serverErrors.rulesManagerRoles ??
    (patch.rules_manager_role_ids
      ? staffRolesProblem(form.rulesManagerRoles, saved.rulesManagerRoles, knownRoles)
      : null);

  const save = useMutation({
    mutationFn: () =>
      write((init) =>
        api.PATCH("/api/guilds/{guild_id}/settings", {
          params: { path: { guild_id: guildId } },
          body: patch,
          ...init,
        }),
      ),
    onSuccess: (next) => {
      queryClient.setQueryData(settingsQuery(guildId).queryKey, next);
      void queryClient.invalidateQueries({ queryKey: healthQuery(guildId).queryKey });
      // Staff roles can change who is staff, the saver included.
      if (patch.moderator_role_ids || patch.rules_manager_role_ids) {
        void queryClient.invalidateQueries({ queryKey: [...keys.guild(guildId), "me"] });
        void queryClient.invalidateQueries({ queryKey: keys.guilds });
      }
      reset();
      setConflict(null);
      toast.success("Settings saved.");
    },
    onError: (e) => {
      if (!(e instanceof ApiError)) {
        setSaveError("Quack couldn't save the settings. Try again in a moment.");
        return;
      }
      if (e.code === "settings_changed") setConflict(e.message);
      const mapped: FieldErrors = {};
      for (const f of e.fields) {
        const key = settingsFields[f.field.replace(/\[\d+\]$/, "")];
        if (key) mapped[key] = mapped[key] ? `${mapped[key]} ${f.message}` : f.message;
      }
      setServerErrors(mapped);
      setSaveError(e.unexpected && e.traceId ? `${e.message} (Trace ID ${e.traceId})` : e.message);
    },
  });

  return (
    <Page
      icon={<SettingsIcon size={22} />}
      title="Settings"
      topic="How Quack works in this server."
      width="narrow"
    >
      {canWrite ? null : (
        <Callout tone="info" icon="lock" title="You can look, but not change">
          To change Quack's settings, you need Manage Server in Discord.
        </Callout>
      )}

      {conflict ? (
        <Callout
          tone="warning"
          icon="warn"
          title="The settings changed while you were editing"
          action={
            <Button
              size="sm"
              variant="secondary"
              onClick={() =>
                void refetch().then(() => {
                  reset();
                  setConflict(null);
                })
              }
            >
              Reload settings
            </Button>
          }
        >
          {conflict} Reloading discards your changes here.
        </Callout>
      ) : null}

      <StarterNotice guildId={guildId} />

      <Health guildId={guildId} />

      <Section title="Staff roles" description="Who can moderate and who can manage rules.">
        <div className={s.fields}>
          <Field
            label="Moderator roles"
            error={moderatorError}
            hint={
              canWriteModerators
                ? "Moderators add and void cases, review appeals, and read the moderation log. With no roles picked, members with Discord's Timeout Members permission can moderate. Once you pick any, only these roles can."
                : "Only the server owner or a member with Administrator can change Quack's moderator roles."
            }
          >
            {(id) => (
              <RolePicker
                id={id}
                guildId={guildId}
                value={form.moderatorRoles}
                onChange={(moderatorRoles) => set({ moderatorRoles })}
                max={maxStaffRoles}
                invalid={Boolean(moderatorError)}
                disabled={!canWriteModerators}
                emptyLabel="None picked. Members with Timeout Members can moderate."
                placeholder="Add a moderator role"
              />
            )}
          </Field>
          {canWriteModerators ? (
            <p className={s.note}>
              Moderators don't need Timeout, Kick, or Ban Members in Discord. Quack acts with its
              own permissions, so only pick roles you trust with every action your rules can take.
            </p>
          ) : null}
          <Field
            label="Rules manager roles"
            error={rulesManagerError}
            hint="Rules managers can create, edit, import, export, and archive rules. Members with Manage Server can too."
          >
            {(id) => (
              <RolePicker
                id={id}
                guildId={guildId}
                value={form.rulesManagerRoles}
                onChange={(rulesManagerRoles) => set({ rulesManagerRoles })}
                max={maxStaffRoles}
                invalid={Boolean(rulesManagerError)}
                disabled={!canWrite}
                emptyLabel="None picked."
                placeholder="Add a rules manager role"
              />
            )}
          </Field>
          <p className={s.note}>
            If this server requires two-factor authentication (2FA) for moderation in Discord, Quack
            requires it for all staff too, the owner included. Staff turn it on in Discord, then
            sign in to this dashboard again.
          </p>
        </div>
      </Section>

      <Divider />

      <Section
        title="Appeals"
        description="Where new appeals appear, and how members who were removed can come back."
      >
        <div className={s.fields}>
          <Field
            label="Appeal channel"
            error={serverErrors.appealChannel}
            hint="Quack posts each new appeal here with Accept and Reject buttons. Pick a channel only staff can see."
          >
            {(id) => (
              <ChannelPicker
                id={id}
                guildId={guildId}
                value={form.appealChannel}
                onChange={(appealChannel) => set({ appealChannel })}
                clearable
                noneLabel="No appeal channel"
                invalid={Boolean(serverErrors.appealChannel)}
                disabled={!canWrite}
              />
            )}
          </Field>
          <Field
            label="Rejoin invite"
            error={inviteError}
            hint="Sent in the DM when an appeal is accepted, so members who were kicked or banned can come back."
          >
            {(id) => (
              <TextInput
                id={id}
                type="url"
                inputMode="url"
                placeholder="https://discord.gg/yourserver"
                value={form.appealInvite}
                invalid={Boolean(inviteError)}
                disabled={!canWrite}
                onChange={(e) => set({ appealInvite: e.currentTarget.value })}
              />
            )}
          </Field>
        </div>
      </Section>

      <Divider />

      <Section
        title="Case DMs"
        description="When a rule's level sends a DM, the member gets one DM per case. You can add a short note of your own; the rest is the same for every server."
      >
        <div className={s.fields}>
          <Field
            label="Introduction"
            error={introError}
            hint={
              <Counter
                value={form.dmIntro}
                max={limits.dmIntro}
                hint="Shown after the action and the appeal line."
              />
            }
          >
            {(id) => (
              <TextArea
                id={id}
                rows={3}
                placeholder="For example: Hi, this is the moderation team of our server."
                value={form.dmIntro}
                invalid={Boolean(introError)}
                disabled={!canWrite}
                onChange={(e) => set({ dmIntro: e.currentTarget.value })}
              />
            )}
          </Field>
          <Field
            label="Footer"
            error={footerError}
            hint={
              <Counter
                value={form.dmFooter}
                max={limits.dmFooter}
                hint="Shown at the end of the DM."
              />
            }
          >
            {(id) => (
              <TextArea
                id={id}
                rows={2}
                placeholder="For example: Questions? Open a ticket in #support."
                value={form.dmFooter}
                invalid={Boolean(footerError)}
                disabled={!canWrite}
                onChange={(e) => set({ dmFooter: e.currentTarget.value })}
              />
            )}
          </Field>
          <DmSample guildName={me.guild.name} introduction={form.dmIntro} footer={form.dmFooter} />
        </div>
      </Section>

      <Divider />

      <Section
        title="Log channel"
        description="Quack posts each moderation log entry here as it happens: new cases, voids, failed actions, appeal decisions, and setting changes. The moderation log on the dashboard keeps every entry either way."
      >
        <Field
          label="Log channel"
          error={serverErrors.logChannel}
          hint={
            <>
              Pick a channel only staff can see. For message edits, deletes, joins, and leaves, use
              the{" "}
              <Link
                to="/guilds/$guildId/modules/activity-log"
                params={{ guildId }}
                className={s.link}
              >
                server activity log
              </Link>{" "}
              instead.
            </>
          }
        >
          {(id) => (
            <ChannelPicker
              id={id}
              guildId={guildId}
              value={form.logChannel}
              onChange={(logChannel) => set({ logChannel })}
              clearable
              noneLabel="Don't post the moderation log"
              invalid={Boolean(serverErrors.logChannel)}
              disabled={!canWrite}
            />
          )}
        </Field>
      </Section>

      <Divider />

      <Section
        title="Evidence channel"
        description="Quack keeps copies of evidence files in a staff-only channel, so they last after the original message is deleted. Uploading files as evidence needs one. Without it, Quack keeps each saved file's details and a link to the original."
      >
        <Field
          label="Evidence channel"
          error={serverErrors.evidenceChannel}
          hint="Pick a channel only staff can see."
        >
          {(id) => (
            <ChannelPicker
              id={id}
              guildId={guildId}
              value={form.evidenceChannel}
              onChange={(evidenceChannel) => set({ evidenceChannel })}
              clearable
              noneLabel="Don't keep copies"
              invalid={Boolean(serverErrors.evidenceChannel)}
              disabled={!canWrite}
            />
          )}
        </Field>
      </Section>

      {canWrite ? (
        <SaveBar
          visible={dirty}
          onReset={reset}
          onSave={() => save.mutate()}
          pending={save.isPending}
          error={saveError}
          saveDisabled={Boolean(
            inviteError || introError || footerError || moderatorError || rulesManagerError,
          )}
        />
      ) : null}
    </Page>
  );
}

function Divider() {
  return <hr className={s.divider} />;
}

function Counter({ value, max, hint }: { value: string; max: number; hint: ReactNode }) {
  const length = Array.from(value.trim()).length;
  return (
    <span className={s.counterRow}>
      <span>{hint}</span>
      {length > 0 ? (
        <span className={s.counter}>
          {length.toLocaleString()} / {max.toLocaleString()}
        </span>
      ) : null}
    </span>
  );
}

const actions: { value: DmAction; label: string }[] = [
  { value: "warning", label: "Warning" },
  { value: "timeout", label: "Timeout" },
  { value: "kick", label: "Kick" },
  { value: "ban", label: "Ban" },
];

const longDate = new Intl.DateTimeFormat(undefined, { dateStyle: "long", timeStyle: "short" });

/** DmSample previews the case DM with the form's introduction and footer. */
function DmSample({
  guildName,
  introduction,
  footer,
}: {
  guildName: string;
  introduction: string;
  footer: string;
}) {
  const [action, setAction] = useState<DmAction>("timeout");
  // Now and a day from when the preview first showed, like a fresh one-day timeout.
  const [times] = useState(() => ({
    added: longDate.format(new Date()),
    ends: longDate.format(new Date(Date.now() + 86_400_000)),
  }));
  const dm = buildCaseDm({
    guildName,
    ruleName: "Spam",
    reason: "Posted the same invite link in several channels.",
    action,
    introduction,
    footer,
    caseNumber: 128,
    addedAt: times.added,
    appealable: true,
    timeoutEnds: { full: times.ends, relative: "in a day" },
    details: [{ label: "Where", value: "#general" }],
  });
  return (
    <div className={s.sample}>
      <Heading
        actions={
          <Segmented label="Sample action" value={action} onChange={setAction} options={actions} />
        }
      >
        Preview
      </Heading>
      <DmPreview dm={dm} />
      <p className={s.note}>
        A sample case. Real DMs use the case's rule, reason, action, and case context.
      </p>
    </div>
  );
}

/**
 * Health shows what Quack needs fixed in this server right now: its
 * permissions, the staff channels, and the modules that are on.
 */
function Health({ guildId }: { guildId: string }) {
  const health = useQuery({ ...healthQuery(guildId), retry: false });
  const checks = health.data ? healthChecklist(health.data) : [];
  const ok = healthy(checks);
  return (
    <Section
      title="Server health"
      description="What Quack can do in this server right now."
      actions={
        health.data ? (
          ok ? (
            <Badge tone="success" dot>
              All good
            </Badge>
          ) : (
            <Badge tone="warning" dot>
              Needs attention
            </Badge>
          )
        ) : null
      }
    >
      {health.isPending ? (
        <SkeletonRows rows={2} />
      ) : health.isError ? (
        <ErrorState error={health.error} retry={() => void health.refetch()} />
      ) : (
        <>
          <ul className={s.checklist}>
            {checks.map((check) => (
              <li key={check.key} className={s.check}>
                <QuackIcon
                  name={
                    check.state === "ok" ? "success" : check.state === "problem" ? "error" : "info"
                  }
                  size={20}
                />
                <div className={s.checkText}>
                  <span className={s.checkLabel}>{check.label}</span>
                  <span className={s.checkDetail}>{check.detail}</span>
                </div>
              </li>
            ))}
          </ul>
          {ok ? null : (
            <p className={s.note}>
              Running <Command>/setup</Command> in Discord checks these again and can create
              staff-only channels for you.
            </p>
          )}
        </>
      )}
    </Section>
  );
}
