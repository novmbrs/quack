import { createFileRoute, Link, redirect } from "@tanstack/react-router";

import { loginUrl } from "~/api/client";
import { authQuery } from "~/api/queries";
import { SiteHeader } from "~/features/site/SiteHeader";
import { useTitle } from "~/features/site/useTitle";
import { QuackIcon } from "~/ui/QuackIcon";

import s from "./login.module.css";

type Search = { redirect?: string; error?: "denied" | "failed" };

export const Route = createFileRoute("/login")({
  validateSearch: (search: Record<string, unknown>): Search => ({
    redirect: typeof search.redirect === "string" ? search.redirect : undefined,
    // The API's sign-in callback comes back here with ?error= when Discord
    // sign-in didn't finish (design §14.4).
    error: search.error === "denied" || search.error === "failed" ? search.error : undefined,
  }),
  beforeLoad: async ({ context, search }) => {
    const me = await context.queryClient.ensureQueryData(authQuery);
    if (me) throw redirect({ href: safeTarget(search.redirect) });
  },
  component: Login,
});

/** signInProblem is the sentence for a sign-in that didn't finish (design §12.14). */
const signInProblem = {
  denied: "Discord sign-in was cancelled. Choose Sign in with Discord when you're ready.",
  failed: "Discord sign-in didn't finish. Choose Sign in with Discord to try again.",
} as const;

/** safeTarget keeps post-sign-in redirects on this site. */
function safeTarget(target: string | undefined): string {
  if (!target || !target.startsWith("/") || target.startsWith("//")) return "/guilds";
  return target;
}

function Login() {
  useTitle("Sign in · Quack");
  const { redirect: target, error } = Route.useSearch();
  return (
    <div className={s.page}>
      <SiteHeader />
      <main className={s.main}>
        <div className={s.panel}>
          <div className={s.text}>
            <h1 className={s.title}>Sign in to Quack</h1>
            <p className={s.subtitle}>Use your Discord account. There's nothing else to set up.</p>
          </div>

          {error ? (
            <p role="alert" className={s.problem}>
              {signInProblem[error]}
            </p>
          ) : null}

          <a href={loginUrl(safeTarget(target))} className={s.discord}>
            <DiscordMark />
            Sign in with Discord
          </a>

          <ul className={s.reasons}>
            <li>
              <QuackIcon name="shield" size={22} />
              <span>
                <b>Staff</b> manage rules, cases, and appeals for their servers.
              </span>
            </li>
            <li>
              <QuackIcon name="appeal" size={22} />
              <span>
                <b>Members</b> see cases about them and appeal them.
              </span>
            </li>
          </ul>

          <p className={s.fine}>
            Quack uses your Discord account to sign you in and check your server access. It never
            posts as you. <Link to="/docs">New to Quack?</Link>
          </p>
        </div>
      </main>
    </div>
  );
}

function DiscordMark() {
  return (
    <svg width="20" height="20" viewBox="0 0 24 24" fill="currentColor" aria-hidden>
      <path d="M20.317 4.37a19.79 19.79 0 0 0-4.885-1.515.074.074 0 0 0-.079.037c-.21.375-.444.864-.608 1.25a18.27 18.27 0 0 0-5.487 0 12.64 12.64 0 0 0-.617-1.25.077.077 0 0 0-.079-.037A19.74 19.74 0 0 0 3.677 4.37a.07.07 0 0 0-.032.027C.533 9.046-.32 13.58.099 18.057a.082.082 0 0 0 .031.057 19.9 19.9 0 0 0 5.993 3.03.078.078 0 0 0 .084-.028c.462-.63.874-1.295 1.226-1.994a.076.076 0 0 0-.041-.106 13.1 13.1 0 0 1-1.872-.892.077.077 0 0 1-.008-.128c.126-.094.252-.192.372-.291a.074.074 0 0 1 .077-.01c3.928 1.793 8.18 1.793 12.062 0a.074.074 0 0 1 .078.009c.12.1.246.198.373.292a.077.077 0 0 1-.006.127 12.3 12.3 0 0 1-1.873.892.077.077 0 0 0-.041.107c.36.698.772 1.362 1.225 1.993a.076.076 0 0 0 .084.028 19.84 19.84 0 0 0 6.002-3.03.077.077 0 0 0 .032-.054c.5-5.177-.838-9.674-3.549-13.66a.061.061 0 0 0-.031-.03ZM8.02 15.33c-1.183 0-2.157-1.085-2.157-2.419 0-1.333.956-2.419 2.157-2.419 1.21 0 2.176 1.096 2.157 2.42 0 1.333-.956 2.418-2.157 2.418Zm7.975 0c-1.183 0-2.157-1.085-2.157-2.419 0-1.333.955-2.419 2.157-2.419 1.21 0 2.176 1.096 2.157 2.42 0 1.333-.946 2.418-2.157 2.418Z" />
    </svg>
  );
}
