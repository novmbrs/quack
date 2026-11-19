import { ApiError } from "~/api/client";
import type { ModuleName, ModuleState } from "~/api/types";

/** moduleWords names each module the way sentences about it do. */
export const moduleWords: Record<
  ModuleName,
  { title: string; noun: string; Noun: string; setupArea: string }
> = {
  tickets: { title: "Tickets", noun: "tickets", Noun: "Tickets", setupArea: "Tickets" },
  activity_log: {
    title: "Server activity log",
    noun: "the server activity log",
    Noun: "The server activity log",
    setupArea: "Server activity log",
  },
  honeypot: {
    title: "Honeypot",
    noun: "the honeypot",
    Noun: "The honeypot",
    setupArea: "Honeypot",
  },
};

/** EnableProblem is why a module couldn't be turned on or off, ready to show. */
export type EnableProblem = {
  /** The API's sentence, such as "Tickets are still off." */
  message: string;
  /** What to fix first, one sentence each (409 module_not_ready). */
  problems: string[];
};

/**
 * enableProblem reads a refused enable or disable. A module that isn't ready
 * comes back as 409 module_not_ready with one problem per thing to fix
 * (design §13.1.7); anything else is shown by its message alone.
 */
export function enableProblem(error: unknown): EnableProblem {
  if (!(error instanceof ApiError)) {
    return { message: "Quack couldn't change this module. Try again in a moment.", problems: [] };
  }
  return { message: error.message, problems: error.problems.map((p) => p.message) };
}

/**
 * disabledReasonText explains why Quack turned a module off by itself, or
 * null when it was turned off by staff (or is on).
 */
export function disabledReasonText(
  module: ModuleName,
  state: Pick<ModuleState, "enabled" | "disabled_reason">,
): string | null {
  if (state.enabled || !state.disabled_reason) return null;
  const { Noun } = moduleWords[module];
  const verb = module === "tickets" ? "were" : "was";
  switch (state.disabled_reason) {
    case "channel_deleted":
      return `${Noun} ${verb} turned off because a channel it uses was deleted. Choose another channel and save, then turn it on again.`;
    case "quack_left":
      return `${Noun} ${verb} turned off because Quack left the server. Turn it on again once Quack is back.`;
  }
}

/**
 * errorText is a failed save as one line for the save bar: the API's
 * message, plus the trace ID when the failure was unexpected.
 */
export function errorText(error: unknown): string | null {
  if (!error) return null;
  if (!(error instanceof ApiError)) return "Quack couldn't save this. Try again in a moment.";
  return error.unexpected && error.traceId
    ? `${error.message} Trace ID ${error.traceId}.`
    : error.message;
}
