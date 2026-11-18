import "./styles/global.css";

import { MutationCache, QueryCache, QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRouter, RouterProvider } from "@tanstack/react-router";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { ApiError } from "./api/client";
import { keys } from "./api/queries";
import { routeTree } from "./routeTree.gen";
import { toast } from "./ui/Toast";

/**
 * signedOut sends the user to sign in again when any request finds the
 * session gone, keeping where they were so they land back on it.
 */
function signedOut(error: unknown) {
  if (!(error instanceof ApiError) || !error.signedOut) return;
  queryClient.setQueryData(keys.auth, null);
  const here = window.location.pathname + window.location.search;
  void router.navigate({ to: "/login", search: { redirect: here } });
}

/** toastMessage is the API's message, with the trace of an unexpected failure. */
function toastMessage(error: unknown): string {
  if (!(error instanceof ApiError)) return "Quack couldn't do that. Try again in a moment.";
  return error.unexpected && error.traceId
    ? `${error.message} (Trace ID ${error.traceId})`
    : error.message;
}

const queryClient = new QueryClient({
  queryCache: new QueryCache({ onError: signedOut }),
  mutationCache: new MutationCache({
    onError: (error, _vars, _ctx, mutation) => {
      signedOut(error);
      // Mutations with their own onError show errors inline instead.
      if (mutation.options.onError) return;
      toast.error(toastMessage(error));
    },
  }),
  defaultOptions: {
    queries: {
      staleTime: 15_000,
      // Client errors (401, 403, 404, 409, 428, 429…) won't change on retry.
      retry: (count, error) =>
        count < 2 && !(error instanceof ApiError && error.status >= 400 && error.status < 500),
      refetchOnWindowFocus: true,
    },
  },
});

const router = createRouter({
  routeTree,
  context: { queryClient },
  // Hovering a link loads its data, so most navigations render instantly.
  defaultPreload: "intent",
  defaultPreloadStaleTime: 0,
  defaultPendingMs: 150,
  defaultPendingMinMs: 0,
  scrollRestoration: true,
});

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  </StrictMode>,
);
