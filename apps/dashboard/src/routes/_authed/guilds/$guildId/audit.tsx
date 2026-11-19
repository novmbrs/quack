import { createFileRoute, redirect } from "@tanstack/react-router";

// The moderation log lived at /audit in the Go dashboard; old links and
// bookmarks still land here.
export const Route = createFileRoute("/_authed/guilds/$guildId/audit")({
  beforeLoad: ({ params }) => {
    throw redirect({ to: "/guilds/$guildId/mod-log", params, replace: true });
  },
});
