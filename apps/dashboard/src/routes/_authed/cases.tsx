import { createFileRoute } from "@tanstack/react-router";

// "Your cases": every case about the signed-in member, across servers.
export const Route = createFileRoute("/_authed/cases")({
  component: () => null,
});
