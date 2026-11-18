import { useQuery, useSuspenseQuery } from "@tanstack/react-query";
import { createFileRoute, Outlet } from "@tanstack/react-router";
import {
  BookOpen,
  FolderOpen,
  LayoutGrid,
  Radar,
  Scale,
  ScrollText,
  Settings,
  Ticket,
  TriangleAlert,
  UserRound,
  Webhook,
} from "lucide-react";

import { ApiError } from "~/api/client";
import { guildMeQuery } from "~/api/queries";
import { showsOverview } from "~/features/shell/guilds";
import { NavGroup, NavItem, Sidebar } from "~/features/shell/Sidebar";
import { useCan } from "~/lib/permissions";
import { Avatar } from "~/ui/Avatar";
import { ButtonLink } from "~/ui/Button";
import { Empty, ErrorState, MfaRequired, SkeletonRows } from "~/ui/States";

import s from "./guild.module.css";

export const Route = createFileRoute("/_authed/guilds/$guildId")({
  loader: ({ context, params }) =>
    context.queryClient.ensureQueryData(guildMeQuery(params.guildId)),
  component: GuildLayout,
  pendingComponent: () => (
    <>
      <Sidebar header="">
        <SkeletonRows rows={8} />
      </Sidebar>
      <main className={s.main} />
    </>
  ),
  errorComponent: ({ error, reset }) => (
    <>
      <Sidebar header="Server">{null}</Sidebar>
      <main className={`${s.main} ${s.center}`}>
        {error instanceof ApiError && error.code === "server_not_found" ? (
          <ServerNotFound message={error.message} />
        ) : (
          <ErrorState error={error} retry={reset} />
        )}
      </main>
    </>
  ),
});

/**
 * ServerNotFound is a server Quack isn't in, or one the account can't see.
 * The API doesn't say which, so neither does the page.
 */
function ServerNotFound({ message }: { message: string }) {
  return (
    <Empty
      icon="search"
      title="Quack couldn't open that server"
      action={
        <div className={s.actions}>
          <ButtonLink to="/guilds" variant="secondary" size="sm">
            Choose a server
          </ButtonLink>
          <ButtonLink to="/cases" variant="secondary" size="sm">
            Your cases
          </ButtonLink>
        </div>
      }
    >
      {message} To see your own cases, open Your cases.
    </Empty>
  );
}

/** NotStaff is a server where the account is a member but has no staff role. */
function NotStaff() {
  return (
    <Empty
      icon="lock"
      title="You aren't staff in this server"
      action={
        <ButtonLink to="/cases" variant="secondary" size="sm">
          Your cases
        </ButtonLink>
      }
    >
      Only moderators, rules managers, and administrators can open a server here. To see cases about
      you, open Your cases.
    </Empty>
  );
}

function GuildLayout() {
  const { guildId } = Route.useParams();
  const { data: me } = useSuspenseQuery(guildMeQuery(guildId));
  const can = useCan(guildId);

  const counts = useQuery({
    ...guildMeQuery(guildId),
    // Keep the appeal and failed action counts fresh while the page is open.
    refetchInterval: 60_000,
  }).data?.counts;

  const params = { guildId };
  const moderates = can("moderate");
  const settings = can("manage_settings");
  const staff = moderates || settings || can("manage_rules") || can("manage_staff_roles");
  return (
    <>
      <Sidebar
        header={
          <>
            <Avatar src={me.guild.icon_url} name={me.guild.name} size={24} square />
            <span className={s.guildName}>{me.guild.name}</span>
          </>
        }
      >
        {showsOverview(me) ? (
          <NavGroup>
            <NavItem
              to="/guilds/$guildId"
              params={params}
              activeOptions={{ exact: true }}
              icon={<LayoutGrid size={18} />}
            >
              Overview
            </NavItem>
          </NavGroup>
        ) : null}
        {moderates ? (
          <NavGroup title="Moderation">
            <NavItem to="/guilds/$guildId/cases" params={params} icon={<FolderOpen size={18} />}>
              Cases
            </NavItem>
            <NavItem
              to="/guilds/$guildId/appeals"
              params={params}
              icon={<Scale size={18} />}
              count={counts?.pending_appeals ?? 0}
            >
              Appeals
            </NavItem>
            <NavItem
              to="/guilds/$guildId/failures"
              params={params}
              icon={<TriangleAlert size={18} />}
              count={counts?.failed_actions ?? 0}
            >
              Failed actions
            </NavItem>
            <NavItem to="/guilds/$guildId/mod-log" params={params} icon={<ScrollText size={18} />}>
              Moderation log
            </NavItem>
          </NavGroup>
        ) : null}
        {moderates || can("manage_rules") ? (
          <NavGroup title="Rules">
            <NavItem to="/guilds/$guildId/rules" params={params} icon={<BookOpen size={18} />}>
              Rules
            </NavItem>
          </NavGroup>
        ) : null}
        {settings || moderates ? (
          <NavGroup title="Server">
            {settings ? (
              <NavItem to="/guilds/$guildId/settings" params={params} icon={<Settings size={18} />}>
                Settings
              </NavItem>
            ) : null}
            <NavItem
              to="/guilds/$guildId/modules/tickets"
              params={params}
              icon={<Ticket size={18} />}
            >
              Tickets
            </NavItem>
            {settings ? (
              <>
                <NavItem
                  to="/guilds/$guildId/modules/activity-log"
                  params={params}
                  icon={<Webhook size={18} />}
                >
                  Server activity log
                </NavItem>
                <NavItem
                  to="/guilds/$guildId/modules/honeypot"
                  params={params}
                  icon={<Radar size={18} />}
                >
                  Honeypot
                </NavItem>
              </>
            ) : null}
          </NavGroup>
        ) : null}
        <NavGroup title="You">
          <NavItem to="/cases" icon={<UserRound size={18} />}>
            Your cases
          </NavItem>
        </NavGroup>
      </Sidebar>
      <main className={me.mfa_blocked || !staff ? `${s.main} ${s.center}` : s.main}>
        {me.mfa_blocked ? <MfaRequired /> : staff ? <Outlet /> : <NotStaff />}
      </main>
    </>
  );
}
