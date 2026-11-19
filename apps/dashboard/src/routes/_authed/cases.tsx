import { useInfiniteQuery } from "@tanstack/react-query";
import { createFileRoute, Link } from "@tanstack/react-router";
import { BookOpen, ChevronRight, Server, UserRound } from "lucide-react";

import { memberCasesQuery } from "~/api/queries";
import type { MemberCaseSummary } from "~/api/types";
import { groupByServer } from "~/features/member/cases";
import { NavGroup, NavItem, Sidebar } from "~/features/shell/Sidebar";
import { appealMeta, fullDate, stamp } from "~/lib/format";
import { Avatar } from "~/ui/Avatar";
import { Badge } from "~/ui/Badge";
import { Button } from "~/ui/Button";
import { Page, Section } from "~/ui/Page";
import { Empty, ErrorState, SkeletonRows } from "~/ui/States";

import s from "./cases.module.css";

// "Your cases": every case about the signed-in member, in every server,
// whether or not Quack's DM reached them.
export const Route = createFileRoute("/_authed/cases")({
  loader: ({ context }) => void context.queryClient.prefetchInfiniteQuery(memberCasesQuery()),
  component: YourCases,
});

function YourCases() {
  const cases = useInfiniteQuery(memberCasesQuery());
  const list = cases.data?.pages.flatMap((p) => p.cases) ?? [];

  return (
    <>
      <Sidebar header="Home">
        <NavGroup>
          <NavItem to="/guilds" icon={<Server size={18} />} activeOptions={{ exact: true }}>
            Your servers
          </NavItem>
          <NavItem to="/cases" icon={<UserRound size={18} />}>
            Your cases
          </NavItem>
        </NavGroup>
        <NavGroup title="Help">
          <NavItem to="/docs/appeals" icon={<BookOpen size={18} />}>
            How appeals work
          </NavItem>
        </NavGroup>
      </Sidebar>
      <main className={s.main}>
        <Page
          icon={<UserRound size={20} />}
          title="Your cases"
          topic="Cases moderators added about you, in every server with Quack."
          width="narrow"
        >
          {cases.isPending ? (
            <SkeletonRows rows={5} />
          ) : cases.isError ? (
            <ErrorState error={cases.error} retry={() => void cases.refetch()} />
          ) : list.length === 0 ? (
            <Empty icon="case" title="No cases about you">
              When a moderator adds a case about you in a server with Quack, it shows up here, even
              if Quack couldn't send you a DM. You can read each case and appeal it when its rule
              allows.
            </Empty>
          ) : (
            <>
              {groupByServer(list).map((group) => (
                <Section
                  key={group.guild.guild_id}
                  title={
                    <span className={s.server}>
                      <Avatar src={group.guild.icon_url} name={group.guild.name} size={24} square />
                      {group.guild.name}
                    </span>
                  }
                >
                  <ul className={s.list}>
                    {group.cases.map((c) => (
                      <CaseRow key={c.id} memberCase={c} />
                    ))}
                  </ul>
                </Section>
              ))}
              {cases.hasNextPage ? (
                <div className={s.more}>
                  <Button
                    variant="secondary"
                    pending={cases.isFetchingNextPage}
                    onClick={() => void cases.fetchNextPage()}
                  >
                    Load more
                  </Button>
                </div>
              ) : null}
            </>
          )}
        </Page>
      </main>
    </>
  );
}

function CaseRow({ memberCase: c }: { memberCase: MemberCaseSummary }) {
  const appeal = c.appeal_state ? appealMeta[c.appeal_state] : null;
  return (
    <li>
      <Link
        to="/guilds/$guildId/cases/$caseId/appeal"
        params={{ guildId: c.guild.guild_id, caseId: c.id }}
        className={s.row}
      >
        <span className={s.text}>
          <span className={s.line}>
            <span className={s.title}>
              Case #{c.number} · {c.rule_name}
            </span>
            <time dateTime={c.created_at} title={fullDate(c.created_at)} className={s.time}>
              {stamp(c.created_at)}
            </time>
          </span>
          <span className={s.sub}>{c.action_text}</span>
          <span className={s.badges}>
            {c.voided_at ? (
              <Badge tone="neutral" dot>
                Voided
              </Badge>
            ) : null}
            {c.appeal_state === "needs_information" ? (
              <Badge tone="info" dot>
                Waiting for your reply
              </Badge>
            ) : appeal ? (
              <Badge tone={appeal.tone} dot>
                Appeal: {appeal.label.toLowerCase()}
              </Badge>
            ) : c.can_appeal ? (
              <Badge tone="brand">You can appeal</Badge>
            ) : null}
          </span>
        </span>
        <ChevronRight size={18} className={s.chevron} />
      </Link>
    </li>
  );
}
