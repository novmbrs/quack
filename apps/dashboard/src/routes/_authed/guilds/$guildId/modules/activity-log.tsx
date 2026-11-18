import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute, Link } from "@tanstack/react-router";
import { Webhook } from "lucide-react";
import { useState } from "react";

import { ApiError, api, write } from "~/api/client";
import { channelsQuery } from "~/api/directory";
import { activityLogModuleQuery, guildMeQuery, healthQuery, keys } from "~/api/queries";
import type { ActivityLogModule } from "~/api/types";
import { ChannelName, ChannelPicker } from "~/features/channels/ChannelPicker";
import {
  type ActivityEvent,
  activityEventGroups,
  activityEvents,
  eventLabel,
  missingDestinations,
  type Routes,
  routesFrom,
  routesToSave,
  sameRoutes,
  withoutEvents,
} from "~/features/modules/activity-log";
import { errorText } from "~/features/modules/enable";
import { Callout, ModuleHeader, StatTiles, useModuleSwitch } from "~/features/modules/ModuleParts";
import { plural } from "~/lib/format";
import { useCan } from "~/lib/permissions";
import { Button } from "~/ui/Button";
import { Field, SwitchRow } from "~/ui/Field";
import { Heading, Page, Section } from "~/ui/Page";
import { SaveBar } from "~/ui/SaveBar";
import { Empty, ErrorState, InlineError, SkeletonRows } from "~/ui/States";
import { toast } from "~/ui/Toast";

import s from "./activity-log.module.css";

export const Route = createFileRoute("/_authed/guilds/$guildId/modules/activity-log")({
  loader: async ({ context, params }) => {
    const me = await context.queryClient.ensureQueryData(guildMeQuery(params.guildId));
    if (me.capabilities.manage_settings) {
      await Promise.all([
        context.queryClient.prefetchQuery(activityLogModuleQuery(params.guildId)),
        context.queryClient.prefetchQuery(channelsQuery(params.guildId)),
      ]);
    }
  },
  component: ActivityLogPage,
});

function ActivityLogPage() {
  const { guildId } = Route.useParams();
  const can = useCan(guildId);
  return (
    <Page
      icon={<Webhook size={22} />}
      title="Server activity log"
      topic="Discord events posted to your staff channels."
      width="narrow"
    >
      {can("manage_settings") ? (
        <ActivityLog guildId={guildId} />
      ) : (
        <Empty icon="lock" title="The server activity log needs Manage Server">
          To change Quack's settings, you need Manage Server in Discord. Ask an administrator to
          check your roles in Server Settings → Roles.
        </Empty>
      )}
    </Page>
  );
}

function ActivityLog({ guildId }: { guildId: string }) {
  const module = useQuery(activityLogModuleQuery(guildId));
  const toggle = useModuleSwitch(guildId, "activity_log");
  const [dirty, setDirty] = useState(false);

  return (
    <>
      <ModuleHeader
        module="activity_log"
        icon="message"
        state={module.data}
        onToggle={toggle.toggle}
        pending={toggle.pending}
        canManage
        locked={
          dirty
            ? "Save or reset your changes before turning the server activity log on or off."
            : null
        }
        problem={toggle.problem}
      >
        Quack posts message edits and deletions, joins and leaves, bans, and server changes to
        channels only your staff can see. It doesn't keep a history of these posts, and it's
        separate from{" "}
        <Link to="/guilds/$guildId/mod-log" params={{ guildId }} className={s.link}>
          the moderation log
        </Link>
        .
      </ModuleHeader>

      {module.isPending ? (
        <SkeletonRows rows={6} />
      ) : module.isError ? (
        <ErrorState error={module.error} retry={() => void module.refetch()} />
      ) : (
        <ActivityLogForm guildId={guildId} data={module.data} onDirtyChange={setDirty} />
      )}
    </>
  );
}

/**
 * Status shows the counters since Quack last started and warns about
 * routes whose channel is gone. Quack stops posting to a deleted channel by
 * itself; clearing the route here just makes the form match.
 */
function Status({
  guildId,
  data,
  routes,
  onClear,
}: {
  guildId: string;
  data: ActivityLogModule;
  routes: Routes;
  onClear: (events: ActivityEvent[]) => void;
}) {
  const channels = useQuery(channelsQuery(guildId));
  const missing = channels.data
    ? missingDestinations(routes, new Set(channels.data.map((c) => c.id)))
    : [];
  const routed = activityEvents.filter((e) => routes[e]).length;

  return (
    <Section title="Status" description="Counts reset when Quack restarts.">
      <StatTiles
        items={[
          {
            label: "Posted",
            value: data.posted_since_start,
            hint: "Events Quack posted since it last started.",
          },
          {
            label: "Dropped",
            value: data.dropped_since_start,
            tone: data.dropped_since_start > 0 ? "warning" : undefined,
            hint: "Events Quack didn't post since it last started, because too many were waiting or their route was removed.",
          },
          {
            label: "Events routed",
            value: `${routed} of ${activityEvents.length}`,
            tone: data.enabled && routed === 0 ? "warning" : undefined,
          },
        ]}
      />
      {missing.map((m) => (
        <Callout
          key={m.channelId}
          tone="danger"
          icon="warn"
          title="A channel for the server activity log is missing"
          action={
            <Button size="sm" variant="secondary" onClick={() => onClear(m.events)}>
              Clear these events
            </Button>
          }
        >
          Quack can't find <ChannelName guildId={guildId} channelId={m.channelId} />, so{" "}
          {plural(m.events.length, "event")} (
          {m.events.map((e) => eventLabel(e).toLowerCase()).join(", ")}) can't be posted. It was
          probably deleted. Choose another channel for them, or clear them and save.
        </Callout>
      ))}
    </Section>
  );
}

type Draft = {
  routes: Routes;
  content: boolean;
  files: boolean;
  embeds: boolean;
};

const draftFrom = (data: ActivityLogModule): Draft => ({
  routes: routesFrom(data.settings.routes),
  content: data.settings.include_message_content,
  files: data.settings.include_file_names,
  embeds: data.settings.include_embed_types,
});

const differs = (a: Draft, b: Draft) =>
  !sameRoutes(a.routes, b.routes) ||
  a.content !== b.content ||
  a.files !== b.files ||
  a.embeds !== b.embeds;

function ActivityLogForm({
  guildId,
  data,
  onDirtyChange,
}: {
  guildId: string;
  data: ActivityLogModule;
  onDirtyChange: (dirty: boolean) => void;
}) {
  const queryClient = useQueryClient();
  const [edits, setEdits] = useState<Draft | null>(null);
  const [saveError, setSaveError] = useState<unknown>(null);
  const base = draftFrom(data);
  const form = edits ?? base;
  const dirty = differs(form, base);

  const set = (patch: Partial<Draft>) => {
    const next = { ...form, ...patch };
    setEdits(next);
    setSaveError(null);
    onDirtyChange(differs(next, base));
  };
  const route = (event: ActivityEvent, channelId: string) =>
    set({ routes: { ...form.routes, [event]: channelId } });
  const reset = () => {
    setEdits(null);
    setSaveError(null);
    onDirtyChange(false);
  };

  const nothingRouted = activityEvents.every((e) => !form.routes[e]);
  const routingError =
    data.enabled && nothingRouted
      ? "Choose a channel for at least one event, or turn the server activity log off first."
      : null;

  const save = useMutation({
    mutationFn: () =>
      write((init) =>
        api.PUT("/api/guilds/{guild_id}/modules/activity-log/settings", {
          params: { path: { guild_id: guildId } },
          body: {
            routes: routesToSave(form.routes),
            include_message_content: form.content,
            include_file_names: form.files,
            include_embed_types: form.embeds,
          },
          ...init,
        }),
      ),
    onSuccess: (result) => {
      queryClient.setQueryData(activityLogModuleQuery(guildId).queryKey, result);
      reset();
      toast.success("Server activity log settings saved.");
      void queryClient.invalidateQueries({ queryKey: keys.modules(guildId) });
      void queryClient.invalidateQueries({ queryKey: healthQuery(guildId).queryKey });
    },
    onError: setSaveError,
  });
  const fieldError = (event: ActivityEvent) =>
    saveError instanceof ApiError ? saveError.field(`routes.${event}`) : undefined;

  return (
    <>
      <Status
        guildId={guildId}
        data={data}
        routes={form.routes}
        onClear={(events) => set({ routes: withoutEvents(form.routes, events) })}
      />

      <Section
        title="Where events go"
        description="Choose a staff-only channel for each kind of event, or leave it off. Quack checks that only staff can see each channel when you save."
      >
        <Field label="Send everything to one channel">
          {(id) => (
            <ChannelPicker
              id={id}
              guildId={guildId}
              value=""
              clearable
              noneLabel="Choose a channel…"
              onChange={(channelId) => {
                if (!channelId) return;
                const routes = {} as Routes;
                for (const e of activityEvents) routes[e] = channelId;
                set({ routes });
              }}
            />
          )}
        </Field>
        {routingError ? <InlineError>{routingError}</InlineError> : null}
        {activityEventGroups.map((group) => (
          <div key={group.title} className={s.group}>
            <Heading>{group.title}</Heading>
            <div className={s.routes}>
              {group.events.map((event) => (
                <div key={event.key} className={s.route}>
                  <div className={s.routeText}>
                    <span className={s.routeLabel}>{event.label}</span>
                    <span className={s.hint}>{event.hint}</span>
                  </div>
                  <ChannelPicker
                    guildId={guildId}
                    value={form.routes[event.key]}
                    onChange={(channelId) => route(event.key, channelId)}
                    clearable
                    noneLabel="Don't post"
                    invalid={Boolean(fieldError(event.key))}
                    className={s.picker}
                  />
                  {fieldError(event.key) ? (
                    <InlineError>{fieldError(event.key)}</InlineError>
                  ) : null}
                </div>
              ))}
            </div>
          </div>
        ))}
      </Section>

      <hr className={s.divider} />

      <Section
        title="Message details"
        description="What Quack includes when it posts an edited or deleted message."
      >
        <div>
          <SwitchRow
            title="Message text"
            description="The words of the message, before and after an edit. Turning this off also stops Quack keeping the text in memory."
            checked={form.content}
            onChange={(content) => set({ content })}
          />
          <SwitchRow
            title="File names"
            description="The names of files attached to the message, linked to Discord's copy. Quack doesn't keep copies of the files."
            checked={form.files}
            onChange={(files) => set({ files })}
          />
          <SwitchRow
            title="Link previews"
            description="Which kinds of link previews the message had, like videos or images."
            checked={form.embeds}
            onChange={(embeds) => set({ embeds })}
          />
        </div>
      </Section>

      <SaveBar
        visible={dirty}
        onReset={reset}
        onSave={() => save.mutate()}
        pending={save.isPending}
        error={errorText(saveError)}
        saveDisabled={Boolean(routingError)}
      />
    </>
  );
}
