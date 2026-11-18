import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import { ExternalLink, Ticket as TicketIcon } from "lucide-react";
import { useState } from "react";

import { ApiError, api, write } from "~/api/client";
import { useCursorPager } from "~/api/paging";
import {
  guildMeQuery,
  healthQuery,
  keys,
  type TicketListState,
  ticketQuery,
  ticketsModuleQuery,
  ticketsQuery,
  ticketTranscriptQuery,
} from "~/api/queries";
import type { Ticket, TicketSettings, TicketState } from "~/api/types";
import { ChannelPicker } from "~/features/channels/ChannelPicker";
import { discordChannelUrl } from "~/features/channels/channels";
import { errorText } from "~/features/modules/enable";
import { Command, ModuleHeader, StatTiles, useModuleSwitch } from "~/features/modules/ModuleParts";
import { UserChip } from "~/features/people/User";
import { ago, fullDate, stamp } from "~/lib/format";
import { useCan } from "~/lib/permissions";
import { Badge, type Tone } from "~/ui/Badge";
import { Button, ExternalButton } from "~/ui/Button";
import { ConfirmDialog } from "~/ui/ConfirmDialog";
import { Pager } from "~/ui/DataTable";
import { Details } from "~/ui/Details";
import { Field, TextInput } from "~/ui/Field";
import { Heading, Page, Panel, Section } from "~/ui/Page";
import { SaveBar } from "~/ui/SaveBar";
import { Segmented } from "~/ui/Segmented";
import { Empty, ErrorState, Skeleton, SkeletonRows } from "~/ui/States";
import { toast } from "~/ui/Toast";

import s from "./tickets.module.css";

export const Route = createFileRoute("/_authed/guilds/$guildId/modules/tickets")({
  loader: async ({ context, params }) => {
    const { queryClient } = context;
    const me = await queryClient.ensureQueryData(guildMeQuery(params.guildId));
    // Warm what this person may see without failing the page over it.
    await Promise.all([
      me.capabilities.manage_settings
        ? queryClient.prefetchQuery(ticketsModuleQuery(params.guildId))
        : null,
      me.capabilities.moderate
        ? queryClient.prefetchQuery(ticketsQuery(params.guildId, "open"))
        : null,
    ]);
  },
  component: TicketsPage,
});

function TicketsPage() {
  const { guildId } = Route.useParams();
  const can = useCan(guildId);
  const canManage = can("manage_settings");
  const canModerate = can("moderate");

  if (!canManage && !canModerate) {
    return (
      <Page icon={<TicketIcon size={22} />} title="Tickets">
        <Empty icon="lock" title="Tickets are for staff">
          Ticket settings need Manage Server in Discord, and the tickets themselves are for
          moderators.
        </Empty>
      </Page>
    );
  }

  return (
    <Page
      icon={<TicketIcon size={22} />}
      title="Tickets"
      topic="Private support threads between a member and your staff."
    >
      <TicketsOverview guildId={guildId} canManage={canManage} canModerate={canModerate} />
    </Page>
  );
}

function TicketsOverview({
  guildId,
  canManage,
  canModerate,
}: {
  guildId: string;
  canManage: boolean;
  canModerate: boolean;
}) {
  const module = useQuery({ ...ticketsModuleQuery(guildId), enabled: canManage });
  const toggle = useModuleSwitch(guildId, "tickets");
  const [dirty, setDirty] = useState(false);

  return (
    <>
      <div className={s.column}>
        <ModuleHeader
          module="tickets"
          icon="ticket"
          state={module.data}
          onToggle={toggle.toggle}
          pending={toggle.pending}
          canManage={canManage}
          locked={dirty ? "Save or reset your changes before turning tickets on or off." : null}
          problem={toggle.problem}
        >
          Members select Open a ticket in your ticket channel to start a private thread with your
          staff. When the ticket closes, Quack saves a transcript and sends the member a copy in a
          DM.
        </ModuleHeader>

        {!canManage ? null : module.isError ? (
          <ErrorState error={module.error} retry={() => void module.refetch()} />
        ) : module.data ? (
          <StatTiles items={[{ label: "Open tickets", value: module.data.open_tickets }]} />
        ) : (
          <Skeleton height={78} />
        )}
      </div>

      {canModerate ? <TicketQueue guildId={guildId} /> : null}

      {canManage ? (
        <div className={s.column}>
          {module.isPending ? (
            <SkeletonRows rows={3} />
          ) : module.isError ? null : (
            <TicketSettingsForm
              guildId={guildId}
              enabled={module.data.enabled}
              saved={module.data.settings}
              onDirtyChange={setDirty}
            />
          )}
        </div>
      ) : null}
    </>
  );
}

type Draft = { entry: string; queue: string; retention: string };

const draftFrom = (saved: TicketSettings): Draft => ({
  entry: saved.entry_channel_id ?? "",
  queue: saved.queue_channel_id ?? "",
  retention: String(saved.transcript_retention_days),
});

const differs = (a: Draft, b: Draft) =>
  a.entry !== b.entry || a.queue !== b.queue || a.retention !== b.retention;

function TicketSettingsForm({
  guildId,
  enabled,
  saved,
  onDirtyChange,
}: {
  guildId: string;
  enabled: boolean;
  saved: TicketSettings;
  onDirtyChange: (dirty: boolean) => void;
}) {
  const queryClient = useQueryClient();
  const [edits, setEdits] = useState<Draft | null>(null);
  const [saveError, setSaveError] = useState<unknown>(null);
  const base = draftFrom(saved);
  const form = edits ?? base;
  const dirty = differs(form, base);

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

  const apiField = (path: string) =>
    saveError instanceof ApiError ? saveError.field(path) : undefined;
  const days = Number(form.retention);
  const retentionValid = Number.isInteger(days) && days >= 1 && days <= 365;
  const retentionError =
    apiField("transcript_retention_days") ?? (retentionValid ? null : "Choose 1 to 365 days");
  const entryProblem =
    enabled && !form.entry ? "Tickets need an entry channel while they're on" : null;
  const queueProblem =
    enabled && !form.queue
      ? "Tickets need a staff channel while they're on"
      : form.queue && form.queue === form.entry
        ? "Use a different channel from the entry channel"
        : null;
  const entryError = apiField("entry_channel_id") ?? entryProblem;
  const queueError = apiField("queue_channel_id") ?? queueProblem;
  const invalid = Boolean(!retentionValid || entryProblem || queueProblem);

  const save = useMutation({
    mutationFn: () =>
      write((init) =>
        api.PUT("/api/guilds/{guild_id}/modules/tickets/settings", {
          params: { path: { guild_id: guildId } },
          body: {
            entry_channel_id: form.entry || null,
            queue_channel_id: form.queue || null,
            transcript_retention_days: days,
          },
          ...init,
        }),
      ),
    onSuccess: (data) => {
      queryClient.setQueryData(ticketsModuleQuery(guildId).queryKey, data);
      void queryClient.invalidateQueries({ queryKey: keys.modules(guildId) });
      void queryClient.invalidateQueries({ queryKey: healthQuery(guildId).queryKey });
      reset();
      toast.success("Ticket settings saved.");
    },
    onError: setSaveError,
  });

  return (
    <Section
      title="Settings"
      description="Where tickets start and where your staff hear about them."
    >
      <div className={s.fields}>
        <Field
          label="Entry channel"
          error={entryError}
          hint={
            <>
              Members open tickets with the button Quack posts here, and their threads live under
              it. When you change it, Quack moves the button. You can also set this up with{" "}
              <Command>/setup</Command> in Discord.
            </>
          }
        >
          {(id) => (
            <ChannelPicker
              id={id}
              guildId={guildId}
              value={form.entry}
              onChange={(entry) => set({ entry })}
              types={["text"]}
              clearable={!enabled}
              noneLabel="No entry channel"
              invalid={Boolean(entryError)}
            />
          )}
        </Field>
        <Field
          label="Staff channel for new tickets"
          error={queueError}
          hint="Quack posts each new ticket here and attaches the transcript when it closes. Choose a channel only staff can see."
        >
          {(id) => (
            <ChannelPicker
              id={id}
              guildId={guildId}
              value={form.queue}
              onChange={(queue) => set({ queue })}
              clearable={!enabled}
              noneLabel="No staff channel"
              invalid={Boolean(queueError)}
            />
          )}
        </Field>
        <Field
          label="Keep transcripts for"
          error={retentionError}
          hint="After this, the transcript is no longer available on the dashboard. Members keep the copy Quack sent them in a DM."
        >
          {(id) => (
            <div className={s.daysRow}>
              <TextInput
                id={id}
                inputMode="numeric"
                value={form.retention}
                invalid={Boolean(retentionError)}
                onChange={(e) => set({ retention: e.currentTarget.value.replace(/\D/g, "") })}
                className={s.days}
              />
              <span className={s.muted}>days</span>
            </div>
          )}
        </Field>
      </div>
      <SaveBar
        visible={dirty}
        onReset={reset}
        onSave={() => save.mutate()}
        pending={save.isPending}
        error={errorText(saveError)}
        saveDisabled={invalid}
      />
    </Section>
  );
}

const stateMeta: Record<TicketState, { label: string; tone: Tone }> = {
  opening: { label: "Opening", tone: "info" },
  open: { label: "Open", tone: "success" },
  closing: { label: "Closing", tone: "warning" },
  closed: { label: "Closed", tone: "neutral" },
};

function TicketQueue({ guildId }: { guildId: string }) {
  const [state, setState] = useState<TicketListState>("open");
  const [selected, setSelected] = useState<string | null>(null);
  const pager = useCursorPager(state);
  const tickets = useQuery(ticketsQuery(guildId, state, pager.cursor));
  const list = tickets.data?.tickets ?? [];
  const current = list.find((t) => t.id === selected) ? selected : (list[0]?.id ?? null);

  return (
    <Section
      title="Tickets"
      description="Oldest first, so whoever has waited longest is on top."
      actions={
        <Segmented
          label="Ticket state"
          value={state}
          onChange={(next) => {
            setState(next);
            setSelected(null);
          }}
          options={[
            { value: "open", label: "Open" },
            { value: "closed", label: "Closed" },
          ]}
        />
      }
    >
      {tickets.isPending ? (
        <SkeletonRows rows={4} />
      ) : tickets.isError ? (
        <ErrorState error={tickets.error} retry={() => void tickets.refetch()} />
      ) : list.length === 0 ? (
        <Panel>
          <Empty
            icon="ticket"
            title={state === "open" ? "No open tickets" : "No closed tickets"}
            compact
          >
            {state === "open"
              ? "When a member opens a ticket, it shows up here."
              : "Closed tickets and their transcripts show up here."}
          </Empty>
        </Panel>
      ) : (
        <>
          <div className={s.split} data-dim={tickets.isPlaceholderData || undefined}>
            <ul className={s.list} aria-label="Tickets">
              {list.map((t) => (
                <li key={t.id}>
                  <button
                    type="button"
                    aria-current={t.id === current || undefined}
                    onClick={() => setSelected(t.id)}
                    className={s.row}
                  >
                    <UserChip guildId={guildId} userId={t.member_id} link={false} />
                    <time
                      dateTime={t.created_at}
                      title={fullDate(t.created_at)}
                      className={s.rowTime}
                    >
                      {ago(t.created_at)}
                    </time>
                  </button>
                </li>
              ))}
            </ul>
            {current ? <TicketDetail key={current} guildId={guildId} ticketId={current} /> : null}
          </div>
          <Pager
            page={pager.page}
            hasPrevious={pager.hasPrevious}
            hasNext={Boolean(tickets.data?.next_cursor)}
            onPrevious={pager.previous}
            onNext={() => pager.next(tickets.data?.next_cursor)}
          />
        </>
      )}
    </Section>
  );
}

/** stateText is the ticket's state in a sentence, like the staff post in Discord (ticket.post.*). */
function stateText(ticket: Ticket): string | null {
  if (ticket.state === "opening") return "Quack is opening this ticket.";
  if (ticket.close_reason === "open_failed")
    return "Quack couldn't open this ticket. Check that Quack can create private threads in the ticket entry channel.";
  if (ticket.state === "closing") return "Closing. Quack is saving the transcript.";
  return null;
}

function TicketDetail({ guildId, ticketId }: { guildId: string; ticketId: string }) {
  const queryClient = useQueryClient();
  const detail = useQuery(ticketQuery(guildId, ticketId));
  const [closing, setClosing] = useState(false);
  const [closeError, setCloseError] = useState<unknown>(null);

  const close = useMutation({
    mutationFn: () =>
      write((init) =>
        api.POST("/api/guilds/{guild_id}/tickets/{ticket_id}/close", {
          params: { path: { guild_id: guildId, ticket_id: ticketId } },
          ...init,
        }),
      ),
    onSuccess: (ticket) => {
      setClosing(false);
      queryClient.setQueryData(ticketQuery(guildId, ticketId).queryKey, ticket);
      toast.success("Quack is closing this ticket and saving the transcript.");
      void queryClient.invalidateQueries({ queryKey: keys.modules(guildId) });
    },
    onError: setCloseError,
  });

  if (detail.isPending) {
    return (
      <Panel>
        <SkeletonRows rows={3} />
      </Panel>
    );
  }
  if (detail.isError) {
    return (
      <Panel>
        <ErrorState error={detail.error} retry={() => void detail.refetch()} />
      </Panel>
    );
  }

  const ticket = detail.data;
  const open = ticket.state === "open";
  const note = stateText(ticket);
  return (
    <Panel className={s.detail}>
      <div className={s.detailHead}>
        <UserChip guildId={guildId} userId={ticket.member_id} size={40} subtitle />
        <Badge tone={stateMeta[ticket.state].tone} dot>
          {stateMeta[ticket.state].label}
        </Badge>
      </div>

      <div className={s.actions}>
        {open ? (
          <>
            {ticket.thread_id ? (
              <ExternalButton
                size="sm"
                variant="secondary"
                href={discordChannelUrl(guildId, ticket.thread_id)}
                target="_blank"
                rel="noreferrer"
                icon={<ExternalLink size={14} />}
              >
                Open thread
              </ExternalButton>
            ) : null}
            <Button size="sm" variant="danger" onClick={() => setClosing(true)}>
              Close ticket
            </Button>
          </>
        ) : null}
      </div>

      {note ? <p className={s.sub}>{note}</p> : null}
      {ticket.thread_delete_failed ? (
        <p className={s.sub}>
          Quack closed the ticket but couldn't delete the thread. Ask someone with Manage Threads to
          delete it.
        </p>
      ) : null}

      <Details
        items={[
          {
            label: "Opened",
            value: (
              <time title={fullDate(ticket.opened_at ?? ticket.created_at)}>
                {stamp(ticket.opened_at ?? ticket.created_at)}
              </time>
            ),
          },
          ticket.closed_at
            ? {
                label: "Closed",
                value: (
                  <span className={s.inline}>
                    <time title={fullDate(ticket.closed_at)}>{stamp(ticket.closed_at)}</time>
                    {ticket.closed_by ? (
                      <>
                        <span className={s.muted}>by</span>
                        <UserChip guildId={guildId} userId={ticket.closed_by} size={20} />
                      </>
                    ) : null}
                  </span>
                ),
              }
            : null,
        ]}
      />

      {ticket.state === "closed" && ticket.close_reason !== "open_failed" ? (
        <Transcript guildId={guildId} ticket={ticket} />
      ) : null}

      <ConfirmDialog
        open={closing}
        onClose={() => {
          setClosing(false);
          setCloseError(null);
        }}
        title="Close this ticket?"
        description="Quack saves a transcript, sends the member a copy in a DM, and deletes the thread. A closed ticket can't be opened again."
        confirmLabel="Close ticket"
        tone="danger"
        pending={close.isPending}
        error={errorText(closeError)}
        onConfirm={() => close.mutate()}
      />
    </Panel>
  );
}

const partialText = {
  history: "Some messages may be missing because Quack couldn't read the full thread.",
  trimmed: "Some messages are missing because Quack reached the 8 MB limit.",
} as const;

/** downloadTranscript saves a transcript as a text file. */
function downloadTranscript(ticketId: string, content: string) {
  const url = URL.createObjectURL(new Blob([content], { type: "text/plain;charset=utf-8" }));
  const a = document.createElement("a");
  a.href = url;
  a.download = `ticket-${ticketId}.txt`;
  document.body.append(a);
  a.click();
  a.remove();
  window.setTimeout(() => URL.revokeObjectURL(url), 1000);
}

function Transcript({ guildId, ticket }: { guildId: string; ticket: Ticket }) {
  const info = ticket.transcript;
  const available = info?.state === "available";
  const transcript = useQuery({ ...ticketTranscriptQuery(guildId, ticket.id), enabled: available });
  const gone =
    info?.state === "expired" ||
    (transcript.error instanceof ApiError && transcript.error.status === 404);
  return (
    <section className={s.block}>
      <Heading
        actions={
          transcript.data && !gone ? (
            <button
              type="button"
              className={s.link}
              onClick={() => downloadTranscript(ticket.id, transcript.data.content)}
            >
              Download
            </button>
          ) : null
        }
      >
        Transcript
      </Heading>
      {!info ? (
        <p className={s.muted}>Quack couldn't save a transcript for this ticket.</p>
      ) : gone ? (
        <p className={s.muted}>The transcript expired on {fullDate(info.expires_at)}.</p>
      ) : transcript.isPending ? (
        <Skeleton height={160} />
      ) : transcript.isError ? (
        <ErrorState error={transcript.error} retry={() => void transcript.refetch()} />
      ) : (
        <>
          <pre className={s.transcript} tabIndex={0}>
            {transcript.data.content.trim() || "The transcript is empty."}
          </pre>
          {info.partial_cause ? <p className={s.sub}>{partialText[info.partial_cause]}</p> : null}
          <p className={s.sub}>
            The transcript is available on the dashboard until{" "}
            {fullDate(transcript.data.expires_at)}.
          </p>
        </>
      )}
    </section>
  );
}
