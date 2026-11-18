import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { type ReactNode, useState } from "react";

import { ApiError, api, write } from "~/api/client";
import { authQuery, keys, memberCaseQuery } from "~/api/queries";
import type { MemberCase } from "~/api/types";
import { cx } from "~/lib/cx";
import { appealMeta, fullDate, stamp } from "~/lib/format";
import { Avatar } from "~/ui/Avatar";
import { Badge } from "~/ui/Badge";
import { Button } from "~/ui/Button";
import { Field, TextArea } from "~/ui/Field";
import { Heading, Panel } from "~/ui/Page";
import { QuackIcon, type QuackIconName } from "~/ui/QuackIcon";

import { memberAppealLine, memberEventTitle, notAppealableLine } from "./copy";
import s from "./Appeal.module.css";

/** APPEAL_LIMIT matches the backend's statement and reply limit (design §3.9). */
const APPEAL_LIMIT = 4000;

type MemberAppeal = NonNullable<MemberCase["appeal"]>;

/**
 * AppealSection is the member's side of an appeal: the form when they can
 * still appeal, otherwise the appeal's state and conversation, with a reply
 * box while the moderators wait on them. Staff are never named.
 */
export function AppealSection({ memberCase: c }: { memberCase: MemberCase }) {
  const [notice, setNotice] = useState<string | null>(null);

  if (c.appeal) {
    return (
      <section className={s.block}>
        <Heading>Your appeal</Heading>
        {notice ? (
          <Callout icon="success" title="Saved" tone="success">
            {notice}
          </Callout>
        ) : null}
        <AppealThread
          appeal={c.appeal}
          memberCase={c}
          onReplied={() =>
            setNotice(
              `Your reply was added to the appeal for case #${c.number}. It's waiting for a decision.`,
            )
          }
        />
      </section>
    );
  }

  if (!c.can_appeal) {
    return (
      <section className={s.block}>
        <Heading>Appeal</Heading>
        <Callout icon="lock" title="This case can't be appealed">
          {notAppealableLine(c)}
        </Callout>
      </section>
    );
  }

  return (
    <section className={s.block}>
      <Heading>Appeal this case</Heading>
      <AppealForm
        memberCase={c}
        onSent={() =>
          setNotice(`Your appeal for case #${c.number} is saved. It's waiting for a decision.`)
        }
      />
    </section>
  );
}

/** errorText is the API's message, with the trace of an unexpected failure. */
function errorText(e: unknown, fallback: string): string {
  if (!(e instanceof ApiError)) return fallback;
  return e.unexpected && e.traceId ? `${e.message} (Trace ID ${e.traceId})` : e.message;
}

function AppealForm({ memberCase: c, onSent }: { memberCase: MemberCase; onSent: () => void }) {
  const queryClient = useQueryClient();
  const [statement, setStatement] = useState("");
  const [error, setError] = useState<string | null>(null);

  const submit = useMutation({
    mutationFn: () =>
      write((init) =>
        api.POST("/api/me/cases/{case_id}/appeal", {
          params: { path: { case_id: c.id } },
          body: { statement: statement.trim() },
          ...init,
        }),
      ),
    onSuccess: (updated) => {
      queryClient.setQueryData(memberCaseQuery(c.id).queryKey, updated);
      void queryClient.invalidateQueries({ queryKey: keys.member });
      onSent();
    },
    onError: (e) => {
      setError(errorText(e, "Quack couldn't save your appeal. Try again."));
      // Already appealed, voided, or no longer appealable: show where it stands.
      if (e instanceof ApiError && e.status === 409) {
        void queryClient.invalidateQueries({ queryKey: memberCaseQuery(c.id).queryKey });
      }
    },
  });

  const length = statement.trim().length;
  return (
    <Panel>
      <form
        className={s.form}
        onSubmit={(e) => {
          e.preventDefault();
          setError(null);
          if (length > 0 && length <= APPEAL_LIMIT) submit.mutate();
        }}
      >
        <p className={s.intro}>
          Explain what you want the moderators to review. You can appeal this case once.
        </p>
        <Field label="Why should moderators reconsider this case?" required error={error}>
          {(id) => (
            <TextArea
              id={id}
              rows={7}
              maxLength={APPEAL_LIMIT}
              value={statement}
              invalid={Boolean(error)}
              placeholder="Explain what happened in your own words."
              onChange={(e) => setStatement(e.currentTarget.value)}
            />
          )}
        </Field>
        <div className={s.formFooter}>
          <Counter length={statement.length} />
          <Button type="submit" disabled={length === 0} pending={submit.isPending}>
            Send appeal
          </Button>
        </div>
      </form>
    </Panel>
  );
}

function AppealThread({
  appeal: a,
  memberCase: c,
  onReplied,
}: {
  appeal: MemberAppeal;
  memberCase: MemberCase;
  onReplied: () => void;
}) {
  const meta = appealMeta[a.state];
  const events = a.events
    .filter((e) => e.kind !== "submitted")
    .sort((x, y) => x.at.localeCompare(y.at));
  return (
    <>
      <Panel className={s.status}>
        <QuackIcon name={meta.icon} size={28} />
        <div className={s.statusText}>
          <div className={s.statusTitle}>
            <Badge tone={meta.tone} dot>
              {a.state === "needs_information" ? "Waiting for your reply" : meta.label}
            </Badge>
          </div>
          <p className={s.statusBody}>{memberAppealLine(c, a)}</p>
        </div>
      </Panel>
      <Panel>
        <ol className={s.thread}>
          <Message
            author="you"
            created={a.created_at}
            title="You appealed the case."
            body={a.statement}
            memberCase={c}
          />
          {events.map((e, i) => (
            <Message
              key={`${e.at}-${i}`}
              author={e.by_you ? "you" : "moderators"}
              created={e.at}
              title={memberEventTitle(e, c)}
              body={e.text}
              memberCase={c}
            />
          ))}
        </ol>
      </Panel>
      {a.can_reply ? <Reply memberCase={c} appeal={a} onReplied={onReplied} /> : null}
    </>
  );
}

/**
 * Message is one turn in the appeal, in Discord's message layout. The
 * moderators' turns show the server, never the person.
 */
function Message({
  author,
  created,
  title,
  body,
  memberCase: c,
}: {
  author: "you" | "moderators";
  created: string;
  title: string;
  body: string;
  memberCase: MemberCase;
}) {
  const { data: me } = useQuery(authQuery);
  const you = me ? me.account.global_name || me.account.username : "You";
  const guild = c.guild.name || "Server";
  return (
    <li className={cx(s.message, author === "moderators" && s.staff)}>
      {author === "you" ? (
        <Avatar src={me?.account.avatar_url} name={you} size={40} />
      ) : (
        <Avatar src={c.guild.icon_url} name={guild} size={40} square />
      )}
      <div className={s.messageBody}>
        <header className={s.messageMeta}>
          <span className={s.author}>{author === "you" ? you : `The moderators of ${guild}`}</span>
          <time dateTime={created} title={fullDate(created)} className={s.time}>
            {stamp(created)}
          </time>
        </header>
        <p className={s.event}>{title}</p>
        {body ? <p className={s.text}>{body}</p> : null}
      </div>
    </li>
  );
}

function Reply({
  memberCase: c,
  appeal: a,
  onReplied,
}: {
  memberCase: MemberCase;
  appeal: MemberAppeal;
  onReplied: () => void;
}) {
  const queryClient = useQueryClient();
  const [text, setText] = useState("");
  const [error, setError] = useState<string | null>(null);

  const send = useMutation({
    mutationFn: () =>
      write((init) =>
        api.POST("/api/me/cases/{case_id}/appeal/information", {
          params: { path: { case_id: c.id } },
          body: { text: text.trim(), expected_version: a.version },
          ...init,
        }),
      ),
    onSuccess: (updated) => {
      queryClient.setQueryData(memberCaseQuery(c.id).queryKey, updated);
      void queryClient.invalidateQueries({ queryKey: keys.member });
      setText("");
      onReplied();
    },
    onError: (e) => {
      setError(errorText(e, "Quack couldn't save your reply. Try again."));
      // The appeal changed while they wrote: load the latest question, keep the draft.
      if (e instanceof ApiError && e.status === 409) {
        void queryClient.invalidateQueries({ queryKey: memberCaseQuery(c.id).queryKey });
      }
    },
  });

  const length = text.trim().length;
  return (
    <Panel>
      <form
        className={s.form}
        onSubmit={(e) => {
          e.preventDefault();
          setError(null);
          if (length > 0 && length <= APPEAL_LIMIT) send.mutate();
        }}
      >
        <Field label="Reply to the moderators" required error={error}>
          {(id) => (
            <TextArea
              id={id}
              rows={5}
              maxLength={APPEAL_LIMIT}
              value={text}
              invalid={Boolean(error)}
              placeholder="Answer what the moderators asked."
              onChange={(e) => setText(e.currentTarget.value)}
            />
          )}
        </Field>
        <div className={s.formFooter}>
          <Counter length={text.length} />
          <Button type="submit" disabled={length === 0} pending={send.isPending}>
            Send reply
          </Button>
        </div>
      </form>
    </Panel>
  );
}

function Counter({ length }: { length: number }) {
  return (
    <span
      aria-live="polite"
      className={s.counter}
      data-near={length > APPEAL_LIMIT * 0.9 || undefined}
    >
      {length.toLocaleString()} / {APPEAL_LIMIT.toLocaleString()}
    </span>
  );
}

/** Callout is a short boxed note with an icon, for states with no list. */
export function Callout({
  icon,
  title,
  tone,
  children,
}: {
  icon: QuackIconName;
  title: string;
  tone?: "success";
  children: ReactNode;
}) {
  return (
    <div className={cx(s.callout, tone === "success" && s.success)}>
      <QuackIcon name={icon} size={24} />
      <div className={s.calloutText}>
        <p className={s.calloutTitle}>{title}</p>
        <p className={s.calloutBody}>{children}</p>
      </div>
    </div>
  );
}
