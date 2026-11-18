import { useSuspenseQuery } from "@tanstack/react-query";
import { useState } from "react";
import { createFileRoute, Link } from "@tanstack/react-router";
import { BookOpen, LifeBuoy, Lock, Plus, Server, UserRound } from "lucide-react";

import { guildsQuery } from "~/api/queries";
import { useSignInAgain } from "~/api/session";
import type { GuildSummary } from "~/api/types";
import { guildRole, installableGuilds, staffGuilds } from "~/features/shell/guilds";
import { NavExternal, NavGroup, NavItem, Sidebar } from "~/features/shell/Sidebar";
import { inviteTo, inviteUrl, supportUrl } from "~/lib/links";
import { Avatar } from "~/ui/Avatar";
import { Button, ButtonLink, ExternalButton } from "~/ui/Button";
import { Page, Section } from "~/ui/Page";
import { Empty } from "~/ui/States";

import s from "./servers.module.css";

export const Route = createFileRoute("/_authed/guilds/")({
  loader: ({ context }) => void context.queryClient.prefetchQuery(guildsQuery),
  component: Servers,
});

function Servers() {
  const { data } = useSuspenseQuery(guildsQuery);
  const staff = staffGuilds(data);
  const ready = staff.filter((g) => !g.mfa_blocked);
  const locked = staff.filter((g) => g.mfa_blocked);
  const missing = installableGuilds(data);

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
          <NavItem to="/docs" icon={<BookOpen size={18} />}>
            Docs
          </NavItem>
          <NavExternal href={supportUrl} icon={<LifeBuoy size={18} />}>
            Support server
          </NavExternal>
          <NavExternal href={inviteUrl} icon={<Plus size={18} />}>
            Add Quack to a server
          </NavExternal>
        </NavGroup>
      </Sidebar>
      <main className={s.main}>
        <Page icon={<Server size={20} />} title="Your servers" width="narrow">
          {ready.length > 0 ? (
            <Section title="Staff">
              <ul className={s.grid}>
                {ready.map((g, i) => (
                  <GuildTile key={g.guild_id} guild={g} index={i} />
                ))}
              </ul>
            </Section>
          ) : locked.length === 0 ? (
            <Empty
              title="No servers to moderate yet"
              action={
                <div className={s.emptyActions}>
                  <ExternalButton href={inviteUrl}>Add Quack to a server</ExternalButton>
                  <ButtonLink to="/cases" variant="secondary">
                    Your cases
                  </ButtonLink>
                  <ButtonLink to="/docs/getting-started" variant="secondary">
                    Setup guide
                  </ButtonLink>
                </div>
              }
            >
              Servers show up here once Quack is in them and you're a moderator, rules manager, or
              administrator there. Looking for cases about you? Open Your cases.
            </Empty>
          ) : null}

          {locked.length > 0 ? <NeedsMfa guilds={locked} /> : null}

          {ready.length > 0 || locked.length > 0 ? (
            <Section
              title="Your cases"
              description="Cases about you in any server with Quack, and your appeals."
              actions={
                <ButtonLink to="/cases" size="sm" variant="secondary">
                  Open Your cases
                </ButtonLink>
              }
            >
              {null}
            </Section>
          ) : null}

          {missing.length > 0 ? (
            <Section
              title="Add Quack"
              description="You have Manage Server in these servers, but Quack isn't in them yet."
            >
              <ul className={s.list}>
                {missing.map((g) => (
                  <li key={g.guild_id} className={s.row}>
                    <Avatar src={g.icon_url} name={g.name} size={36} square />
                    <span className={s.rowName}>{g.name}</span>
                    <ExternalButton href={inviteTo(g.guild_id)} variant="secondary" size="sm">
                      Add Quack
                    </ExternalButton>
                  </li>
                ))}
              </ul>
            </Section>
          ) : null}
        </Page>
      </main>
    </>
  );
}

/**
 * NeedsMfa lists servers that require 2FA for moderation where Quack hasn't
 * confirmed the account's. They can't open until the member signs in again.
 */
function NeedsMfa({ guilds }: { guilds: GuildSummary[] }) {
  const signInAgain = useSignInAgain();
  const [pending, setPending] = useState(false);
  return (
    <Section
      title="Needs 2FA"
      description="These servers require two-factor authentication (2FA) for moderation. In Discord, open User Settings → My Account and turn on two-factor authentication. Then sign in again here."
      actions={
        <Button
          size="sm"
          variant="secondary"
          pending={pending}
          onClick={() => {
            setPending(true);
            void signInAgain();
          }}
        >
          Sign in again
        </Button>
      }
    >
      <ul className={s.list}>
        {guilds.map((g) => {
          const name = g.name;
          return (
            <li key={g.guild_id} className={s.row} data-locked>
              <Avatar src={g.icon_url} name={name} size={36} square />
              <span className={s.rowName}>{name}</span>
              <span className={s.locked}>
                <Lock size={14} aria-hidden /> Needs 2FA
              </span>
            </li>
          );
        })}
      </ul>
    </Section>
  );
}

function GuildTile({ guild, index }: { guild: GuildSummary; index: number }) {
  const name = guild.name;
  return (
    <li style={{ animationDelay: `${index * 30}ms` }} className={s.item}>
      <Link to="/guilds/$guildId" params={{ guildId: guild.guild_id }} className={s.tile}>
        <Avatar src={guild.icon_url} name={name} size={56} square />
        <span className={s.text}>
          <span className={s.name}>{name}</span>
          <span className={s.role}>{guildRole(guild)}</span>
        </span>
      </Link>
    </li>
  );
}
