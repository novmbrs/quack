import {
  useInfiniteQuery,
  useQuery,
  useQueryClient,
  useSuspenseQuery,
} from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import { FileText, FolderOpen } from "lucide-react";
import type { ReactNode } from "react";

import { ApiError, api } from "~/api/client";
import { authQuery, memberCaseQuery, memberCasesQuery } from "~/api/queries";
import type { MemberCase } from "~/api/types";
import { Timeline } from "~/features/cases/Timeline";
import { AppealSection } from "~/features/member/Appeal";
import { MemberEvidence } from "~/features/member/Evidence";
import { NavGroup, NavItem, Sidebar } from "~/features/shell/Sidebar";
import { fullDate, stamp } from "~/lib/format";
import { Avatar } from "~/ui/Avatar";
import { Badge } from "~/ui/Badge";
import { Button } from "~/ui/Button";
import { Details } from "~/ui/Details";
import { Heading, Page, Panel } from "~/ui/Page";
import { QuackIcon } from "~/ui/QuackIcon";
import { Empty, ErrorState, SkeletonRows } from "~/ui/States";

import s from "./appeal.module.css";

// The trailing underscore on $guildId_ keeps this page out of the staff
// server layout, which checks staff capabilities. Members, including banned
// ones, land here from Quack's DMs. The page ignores the server segment
// (old DMs carry Quack's internal ID there) and finds the case by its ID.
export const Route = createFileRoute("/_authed/guilds/$guildId_/cases/$caseId/appeal")({
  loader: async ({ context, params }) => {
    const c = await context.queryClient.ensureQueryData(memberCaseQuery(params.caseId));
    void context.queryClient.prefetchInfiniteQuery(memberCasesQuery(c.guild.guild_id));
  },
  component: MemberCasePage,
  pendingComponent: () => (
    <MemberLayout>
      <Page icon={<FolderOpen size={20} />} title="Case" width="narrow">
        <SkeletonRows rows={6} />
      </Page>
    </MemberLayout>
  ),
  errorComponent: ({ error, reset }) => (
    <MemberLayout>
      <Page icon={<FolderOpen size={20} />} title="Case" width="narrow">
        {error instanceof ApiError && (error.status === 404 || error.status === 403) ? (
          <NotYours />
        ) : (
          <ErrorState error={error} retry={reset} />
        )}
      </Page>
    </MemberLayout>
  ),
});

/**
 * MemberLayout is the member's frame: a sidebar listing their cases in the
 * case's server instead of the staff navigation.
 */
function MemberLayout({ children }: { children: ReactNode }) {
  const { caseId } = Route.useParams();
  const current = useQuery({ ...memberCaseQuery(caseId), retry: false });
  const guild = current.data?.guild;
  const cases = useInfiniteQuery({
    ...memberCasesQuery(guild?.guild_id),
    enabled: Boolean(guild),
  });
  const list = cases.data?.pages.flatMap((p) => p.cases) ?? [];

  return (
    <>
      <Sidebar
        header={
          guild ? (
            <>
              <Avatar src={guild.icon_url} name={guild.name || "Server"} size={24} square />
              <span className={s.guildName}>{guild.name || "Server"}</span>
            </>
          ) : (
            "Your cases"
          )
        }
      >
        {guild && cases.isPending ? (
          <SkeletonRows rows={3} />
        ) : (
          <NavGroup title="Your cases">
            {list.map((k) => (
              <NavItem
                key={k.id}
                to="/guilds/$guildId/cases/$caseId/appeal"
                params={{ guildId: k.guild.guild_id, caseId: k.id }}
                icon={<FileText size={20} />}
                count={k.appeal_state === "needs_information" ? 1 : 0}
                countTone="brand"
              >
                #{k.number} {k.rule_name}
              </NavItem>
            ))}
            <NavItem to="/cases" icon={<FolderOpen size={20} />}>
              All your cases
            </NavItem>
          </NavGroup>
        )}
      </Sidebar>
      <main className={s.main}>{children}</main>
    </>
  );
}

function MemberCasePage() {
  const { caseId } = Route.useParams();
  const { data: c } = useSuspenseQuery(memberCaseQuery(caseId));
  const guild = c.guild.name || "Server";
  const voided = Boolean(c.voided_at);

  return (
    <MemberLayout>
      <Page
        icon={<FolderOpen size={20} />}
        title={`Case #${c.number}`}
        topic={guild}
        width="narrow"
      >
        {voided ? (
          <div className={s.voidBanner}>
            <QuackIcon name="case_void" size={24} />
            <div>
              <p className={s.bannerTitle}>This case was voided. It no longer counts.</p>
              {c.void_reason ? <p className={s.bannerBody}>{c.void_reason}</p> : null}
            </div>
          </div>
        ) : null}

        <Panel className={s.hero}>
          <div className={s.heroTop}>
            <div className={s.guild}>
              <Avatar src={c.guild.icon_url} name={guild} size={40} square />
              <div className={s.guildText}>
                <span className={s.guildTitle}>{guild}</span>
                <span className={s.sub}>
                  Case #{c.number} ·{" "}
                  <time dateTime={c.created_at} title={fullDate(c.created_at)}>
                    {stamp(c.created_at)}
                  </time>
                </span>
              </div>
            </div>
            {voided ? (
              <Badge tone="neutral" dot>
                Voided
              </Badge>
            ) : null}
          </div>
          <div className={s.reason}>
            <span className={s.quoteBar} />
            <div>
              <p className={s.reasonLabel}>Rule: {c.rule_name}</p>
              <p className={s.reasonText}>{c.reason}</p>
            </div>
          </div>
          <Details items={outcomeItems(c)} />
        </Panel>

        <AppealSection memberCase={c} />

        {c.context.length > 0 ? (
          <section className={s.block}>
            <Heading>Case context</Heading>
            <Panel>
              <Details
                items={c.context.map((v) => ({
                  label: v.label,
                  value: <span className={s.pre}>{v.value}</span>,
                }))}
              />
            </Panel>
          </section>
        ) : null}

        {c.evidence.length ? (
          <section className={s.block}>
            <Heading>Evidence</Heading>
            {c.evidence.map((e) => (
              <MemberEvidence key={e.id} evidence={e} caseId={c.id} />
            ))}
          </section>
        ) : null}

        {c.timeline.length ? (
          <section className={s.block}>
            <Heading>History</Heading>
            <Panel>
              <Timeline items={c.timeline} />
              {c.timeline_truncated ? (
                <p className={s.sub}>Only the most recent history is shown.</p>
              ) : null}
            </Panel>
          </section>
        ) : null}
      </Page>
    </MemberLayout>
  );
}

/**
 * outcomeItems is what the case did, in the server's own sentences: the
 * action, the DM, an undo, and any case it replaced or was replaced by.
 */
function outcomeItems(c: MemberCase) {
  const items: { label: string; value: ReactNode }[] = [{ label: "Action", value: c.action_text }];
  if (c.timeout_ends_at) {
    items.push({ label: "Timeout ends", value: fullDate(c.timeout_ends_at) });
  }
  if (c.undo_text) items.push({ label: "Undo", value: c.undo_text });
  items.push({ label: "DM", value: c.dm_text });
  if (c.replaces_case_number) {
    items.push({ label: "Replaces", value: `Case #${c.replaces_case_number}` });
  }
  if (c.replaced_by_case_number) {
    items.push({ label: "Replaced by", value: `Case #${c.replaced_by_case_number}` });
  }
  return items;
}

/**
 * NotYours covers both a missing case and someone else's: the API answers
 * the same way for each so case IDs can't be probed.
 */
function NotYours() {
  const { data: me } = useQuery(authQuery);
  const queryClient = useQueryClient();
  const name = me ? me.account.global_name || me.account.username : "this account";

  const switchAccount = async () => {
    const here = window.location.pathname + window.location.search;
    await api.POST("/api/auth/logout").catch(() => undefined);
    queryClient.clear();
    window.location.assign(`/login?${new URLSearchParams({ redirect: here })}`);
  };

  return (
    <Empty
      icon="lock"
      title="Quack couldn't find this case for your account"
      action={
        <Button variant="secondary" onClick={switchAccount}>
          Sign out
        </Button>
      }
    >
      You're signed in as {name}. The case doesn't exist, or it's about another Discord account. If
      Quack sent the DM to a different account, sign out, switch accounts in Discord, then open the
      link again.
    </Empty>
  );
}
