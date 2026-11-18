import type { ContextInput, ContextValue, Field } from "~/api/types";

/** ContextValues are a case context form's answers, keyed by field key. */
export type ContextValues = Record<string, string | boolean>;

/** DETAILS is the free-text field a rule without fields still takes (design §3.2). */
export const DETAILS: Field = {
  key: "details",
  label: "Details",
  kind: "long_text",
  required: false,
};

/** contextFields are the questions to ask for a rule: its fields, or just Details. */
export function contextFields(fields: Field[]): Field[] {
  return fields.length > 0 ? fields : [DETAILS];
}

export function isBlank(v: string | boolean | undefined): boolean {
  return v === undefined || (typeof v === "string" && v.trim() === "");
}

/**
 * contextPayload turns form answers into the API's context object. Blank
 * answers are left out; numbers stay text so the server parses them exactly.
 */
export function contextPayload(
  fields: Field[],
  values: ContextValues,
): Record<string, ContextInput> {
  const out: Record<string, ContextInput> = {};
  for (const f of fields) {
    const v = values[f.key];
    if (f.kind === "boolean") {
      // A switch always has an answer: off is No.
      out[f.key] = v === true;
      continue;
    }
    if (isBlank(v)) continue;
    out[f.key] = String(v).trim();
  }
  return out;
}

/** contextValues turns a saved case context back into form answers. */
export function contextValues(context: Record<string, ContextValue>): ContextValues {
  return { ...context };
}

/** missingRequired lists required fields without an answer. */
export function missingRequired(fields: Field[], values: ContextValues): Field[] {
  return fields.filter((f) => f.required && f.kind !== "boolean" && isBlank(values[f.key]));
}

/** evidenceLinks splits pasted text into message links, one per line or space. */
export function evidenceLinks(text: string): string[] {
  return text
    .split(/\s+/)
    .map((l) => l.trim())
    .filter(Boolean);
}
