import type { MemberCase } from "~/api/types";

type MemberAppeal = NonNullable<MemberCase["appeal"]>;
type MemberAppealEvent = MemberAppeal["events"][number];

/**
 * memberAppealLine says where the member's appeal stands, in the words of
 * the appeal DMs (design §12.8, §12.9).
 */
export function memberAppealLine(c: Pick<MemberCase, "number">, a: MemberAppeal) {
  const n = c.number;
  switch (a.state) {
    case "pending":
      return `Your appeal for case #${n} is waiting for a decision. Updates show up here.`;
    case "needs_information":
      return "The moderators asked for more information. Read their question and reply below.";
    case "accepted":
      return `The moderators accepted your appeal. Case #${n} no longer counts.`;
    case "rejected":
      return `The moderators rejected your appeal. Case #${n} still counts.`;
    case "closed":
      return a.events.some((e) => e.kind === "closed" && e.closed_by_void)
        ? `Your appeal was closed because case #${n} was voided. The case no longer counts.`
        : `The moderators closed your appeal. Case #${n} still counts.`;
  }
}

/**
 * memberEventTitle is one appeal event in the member view (design §12.9
 * appeal.event, member column). The moderators are never named.
 */
export function memberEventTitle(e: Pick<MemberAppealEvent, "kind" | "closed_by_void">): string {
  switch (e.kind) {
    case "submitted":
      return "You appealed the case.";
    case "information_requested":
      return "The moderators asked for more information.";
    case "information_provided":
      return "You replied to the appeal.";
    case "accepted":
      return "The moderators accepted the appeal. The case was voided and no longer counts.";
    case "rejected":
      return "The moderators rejected the appeal. The case still counts.";
    case "closed":
      return e.closed_by_void
        ? "The appeal was closed because the case was voided."
        : "The moderators closed the appeal.";
    case "reopened":
      return "The moderators reopened the appeal and asked for more information.";
  }
}

/** notAppealableLine explains why a case without an appeal can't get one. */
export function notAppealableLine(c: Pick<MemberCase, "number" | "voided_at">): string {
  return c.voided_at
    ? `Case #${c.number} was voided and no longer counts.`
    : "This case can't be appealed.";
}
