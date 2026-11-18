/** DmAction is the action a sample case DM describes; warning means none. */
export type DmAction = "warning" | "timeout" | "kick" | "ban";

/** Segment is a run of DM text: plain, bold, or one of Discord's timestamps. */
export type Segment = { text: string; strong?: boolean; time?: boolean };

/**
 * CaseDm is a case DM in the layout of design §12.8 (dm.case): a heading
 * with the case number and server, the rule and date, the reason quoted,
 * then the action, the appeal line, the server's introduction, the case
 * context, and the server's footer, with the DM's buttons.
 */
export type CaseDm = {
  heading: string;
  rule: Segment[];
  quote: string;
  paragraphs: Segment[][];
  buttons: string[];
};

/** CaseDmInput is the case and server text a case DM is built from. */
export type CaseDmInput = {
  guildName: string;
  ruleName: string;
  reason: string;
  action: DmAction;
  introduction: string;
  footer: string;
  caseNumber: number;
  /** When the case was added, as Discord shows a full timestamp. */
  addedAt: string;
  appealable: boolean;
  /** How the timeout's end reads: the full date, then relative. */
  timeoutEnds?: { full: string; relative: string };
  /** Case context lines (message links are never shown in DMs). */
  details?: { label: string; value: string }[];
};

const actionLines: Record<Exclude<DmAction, "warning" | "timeout">, string> = {
  kick: "The rule calls for removing you from the server. Open your case on the dashboard to check its outcome.",
  ban: "The rule calls for banning you from the server. Open your case on the dashboard to check its outcome.",
};

function actionLine(input: CaseDmInput): Segment[] {
  const label: Segment = { text: "Action for this case: ", strong: true };
  switch (input.action) {
    case "warning":
      return [label, { text: "This case is a warning." }];
    case "timeout":
      return input.timeoutEnds
        ? [
            label,
            { text: "The rule calls for a timeout until " },
            { text: input.timeoutEnds.full, time: true },
            { text: " (" },
            { text: input.timeoutEnds.relative, time: true },
            { text: "). Open your case on the dashboard to check its outcome." },
          ]
        : [
            label,
            {
              text: "The rule calls for a timeout. Open your case on the dashboard to check its outcome.",
            },
          ];
    default:
      return [label, { text: actionLines[input.action] }];
  }
}

/**
 * buildCaseDm lays out the DM a member gets for a case (design §12.8). The
 * action line describes what the rule calls for, never Discord's result,
 * since the DM can arrive before a kick or ban runs.
 */
export function buildCaseDm(input: CaseDmInput): CaseDm {
  const server = input.guildName.trim() || "this server";
  const paragraphs: Segment[][] = [actionLine(input)];
  paragraphs.push([
    {
      text: input.appealable
        ? "To ask the moderators to review this case, select Appeal case. You can appeal once."
        : "This case can't be appealed.",
    },
  ]);
  const introduction = input.introduction.trim();
  if (introduction) paragraphs.push([{ text: introduction }]);
  const details = input.details ?? [];
  if (details.length > 0) {
    paragraphs.push([{ text: "Details", strong: true }]);
    for (const d of details) {
      const value = Array.from(d.value);
      const cut = value.length > 300 ? `${value.slice(0, 299).join("")}…` : d.value;
      paragraphs.push([{ text: `${d.label}: `, strong: true }, { text: cut }]);
    }
  }
  const footer = input.footer.trim();
  if (footer) paragraphs.push([{ text: footer }]);

  return {
    heading: `Case #${input.caseNumber} in ${server}`,
    rule: [
      { text: "Rule: " },
      { text: input.ruleName.trim() || "a rule", strong: true },
      { text: " · " },
      { text: input.addedAt, time: true },
    ],
    quote: input.reason,
    paragraphs,
    buttons: input.appealable ? ["Appeal case", "Open your case"] : ["Open your case"],
  };
}
