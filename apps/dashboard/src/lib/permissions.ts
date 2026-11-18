import { useSuspenseQuery } from "@tanstack/react-query";

import { guildMeQuery } from "~/api/queries";
import type { Capabilities } from "~/api/types";

/**
 * Capability is one of the four things staff can do in a server (design
 * §2.1): moderate (moderators), manage_rules (rules managers),
 * manage_settings (administrators), and manage_staff_roles.
 */
export type Capability = keyof Capabilities;

/**
 * useCan reports what the signed-in account may do in a server, after the
 * 2FA rule. It only shapes the UI; the API checks again on every request.
 */
export function useCan(guildId: string) {
  const { data } = useSuspenseQuery(guildMeQuery(guildId));
  return (capability: Capability) => data.capabilities[capability] === true;
}
