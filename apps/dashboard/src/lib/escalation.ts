/**
 * selectLevel mirrors the backend's level choice (design §3.4) so the
 * landing page demo can show escalation: the level with the highest
 * from_case_count at or below the case count wins, else the first level.
 * Real cases use the server's preview (GET …/cases/preview).
 */
export function selectLevel<L extends { from_case_count: number }>(
  levels: readonly L[],
  caseCount: number,
): L | undefined {
  let best: L | undefined;
  for (const level of levels) {
    if (level.from_case_count > caseCount) continue;
    if (!best || level.from_case_count > best.from_case_count) best = level;
  }
  return best ?? levels[0];
}
