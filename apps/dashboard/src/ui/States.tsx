import { type ReactNode, useState } from "react";

import { ApiError } from "~/api/client";
import { useSignInAgain } from "~/api/session";
import { cx } from "~/lib/cx";

import { Button } from "./Button";
import { Logo } from "./Logo";
import { QuackIcon, type QuackIconName } from "./QuackIcon";
import s from "./States.module.css";

/** Empty explains why a list has nothing in it and what to do next. */
export function Empty({
  icon,
  title,
  children,
  action,
  compact,
}: {
  /** A Signals icon; without one the Quack logo shows. */
  icon?: QuackIconName;
  title: ReactNode;
  children?: ReactNode;
  action?: ReactNode;
  compact?: boolean;
}) {
  return (
    <div className={cx(s.empty, compact && s.compact)}>
      {icon ? (
        <QuackIcon name={icon} size={compact ? 32 : 44} />
      ) : (
        <Logo size={compact ? 40 : 60} />
      )}
      <div className={s.text}>
        <p className={s.title}>{title}</p>
        {children ? <p className={s.body}>{children}</p> : null}
      </div>
      {action}
    </div>
  );
}

/**
 * errorMessage is the sentence to show for a failure: the API's own message
 * as is, or a plain fallback for anything that isn't an API error.
 */
export function errorMessage(error: unknown): string {
  return error instanceof ApiError
    ? error.message
    : "Quack couldn't load this. Try again in a moment.";
}

/**
 * TraceId shows the trace of an unexpected failure (network or server
 * error) in small print, so a report can be matched to its trace.
 */
export function TraceId({ error }: { error: unknown }) {
  if (!(error instanceof ApiError) || !error.unexpected || !error.traceId) return null;
  return <span className={s.trace}>Trace ID {error.traceId}</span>;
}

/**
 * ErrorState shows a failed load with the API's message and a retry. A
 * server's 2FA requirement gets its own explanation instead.
 */
export function ErrorState({ error, retry }: { error: unknown; retry?: () => void }) {
  if (error instanceof ApiError && error.code === "mfa_required") return <MfaRequired />;
  const status = error instanceof ApiError ? error.status : 0;
  const forbidden = status === 403;
  const missing = status === 404;
  return (
    <Empty
      icon={forbidden ? "lock" : missing ? "search" : "error"}
      title={forbidden ? "You can't open this" : missing ? "Nothing here" : "Quack couldn't load this"}
      action={
        retry && !forbidden && !missing ? (
          <Button variant="secondary" size="sm" onClick={retry}>
            Try again
          </Button>
        ) : null
      }
    >
      {errorMessage(error)} <TraceId error={error} />
    </Empty>
  );
}

/**
 * MfaRequired explains that the server requires 2FA for moderation and
 * Quack hasn't confirmed the user has it. Quack only learns that when they
 * sign in, so the way back is to turn 2FA on, then sign in again.
 */
export function MfaRequired() {
  const signInAgain = useSignInAgain();
  const [pending, setPending] = useState(false);
  return (
    <Empty
      icon="lock"
      title="This server requires 2FA"
      action={
        <Button
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
      This server requires two-factor authentication (2FA) for moderation. In Discord, open User
      Settings → My Account and turn on two-factor authentication. Then sign in again here.
    </Empty>
  );
}

/**
 * InlineError is a failed action's message, shown where the action was.
 * Pass error to show the API's message (and the trace of an unexpected
 * failure), or children for a message of the page's own.
 */
export function InlineError({ children, error }: { children?: ReactNode; error?: unknown }) {
  return (
    <p role="alert" className={s.inlineError}>
      {children ?? errorMessage(error)} {error ? <TraceId error={error} /> : null}
    </p>
  );
}

/** Skeleton is a placeholder bar shown while content loads. */
export function Skeleton({
  width = "100%",
  height = 16,
  round,
}: {
  width?: number | string;
  height?: number;
  round?: boolean;
}) {
  return <span className={cx(s.skeleton, round && s.round)} style={{ width, height }} />;
}

/** SkeletonRows fills a list area while the first page loads. */
export function SkeletonRows({ rows = 6 }: { rows?: number }) {
  return (
    <div className={s.rows} aria-busy>
      {Array.from({ length: rows }, (_, i) => (
        <div key={i} className={s.row}>
          <Skeleton width={32} height={32} round />
          <div className={s.rowText}>
            <Skeleton width={`${40 + ((i * 17) % 35)}%`} height={12} />
            <Skeleton width={`${20 + ((i * 11) % 25)}%`} height={10} />
          </div>
        </div>
      ))}
    </div>
  );
}
