import { createFileRoute, redirect } from "@tanstack/react-router";

// The server activity log lived at /modules/logging in the Go dashboard; old
// links and bookmarks still land here.
export const Route = createFileRoute("/_authed/guilds/$guildId/modules/logging")({
  beforeLoad: ({ params }) => {
    throw redirect({ to: "/guilds/$guildId/modules/activity-log", params, replace: true });
  },
});
