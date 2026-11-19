import type { Bucket } from "~/api/types";

/** fillDays turns sparse cases_by_day buckets (keyed YYYY-MM-DD) into one entry per UTC day. */
export function fillDays(
  buckets: readonly Pick<Bucket, "key" | "count">[],
  from: Date,
  days: number,
): { day: string; count: number }[] {
  const counts = new Map(buckets.map((b) => [b.key.slice(0, 10), b.count]));
  const out: { day: string; count: number }[] = [];
  for (let i = 0; i < days; i++) {
    const day = new Date(from.getTime() + i * 86400_000).toISOString().slice(0, 10);
    out.push({ day, count: counts.get(day) ?? 0 });
  }
  return out;
}

/** total adds up a group of buckets. */
export function total(buckets: readonly Pick<Bucket, "count">[] | undefined): number {
  return (buckets ?? []).reduce((sum, b) => sum + b.count, 0);
}
