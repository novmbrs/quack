import type { CasePreview } from "~/api/types";
import { capitalize, countedCase, last, levelName, levelOutcome } from "~/lib/format";

/**
 * describePreview words the server's case preview (design §14.3 route 58):
 * what the level does, which level it is, and which counted case this is.
 */
export function describePreview(p: CasePreview): { title: string; level: string; detail: string } {
  const window = p.counting_window_days
    ? `, counting cases from the last ${last(p.counting_window_days * 86400)}`
    : "";
  const dm = p.level.send_dm ? "The member gets a DM." : "This level doesn't send a DM.";
  return {
    title: capitalize(levelOutcome(p.level.action)),
    level: levelName(p.level_index, p.level.from_case_count),
    detail: `This will be ${countedCase(p.case_count)}${window}. ${dm}`,
  };
}
