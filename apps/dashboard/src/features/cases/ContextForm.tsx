import type { Field as RuleField } from "~/api/types";
import { Field, Switch, TextArea, TextInput } from "~/ui/Field";

import type { ContextValues } from "./context";

import s from "./CreateCaseDialog.module.css";

/**
 * ContextForm asks a rule's case context questions. Errors are the API's
 * messages for each field, keyed by field key.
 */
export function ContextForm({
  fields,
  values,
  onChange,
  errors,
}: {
  fields: RuleField[];
  values: ContextValues;
  onChange: (values: ContextValues) => void;
  errors?: (key: string) => string | undefined;
}) {
  return (
    <>
      {fields.map((field) => (
        <ContextInput
          key={field.key}
          field={field}
          value={values[field.key]}
          error={errors?.(field.key)}
          onChange={(v) => onChange({ ...values, [field.key]: v })}
        />
      ))}
    </>
  );
}

function ContextInput({
  field,
  value,
  error,
  onChange,
}: {
  field: RuleField;
  value: string | boolean | undefined;
  error?: string;
  onChange: (v: string | boolean) => void;
}) {
  if (field.kind === "boolean") {
    return (
      <div className={s.boolRow}>
        <span className={s.boolLabel}>{field.label}</span>
        <Switch label={field.label} checked={value === true} onChange={onChange} />
        {error ? (
          <p role="alert" className={s.error}>
            {error}
          </p>
        ) : null}
      </div>
    );
  }
  const text = typeof value === "string" ? value : "";
  return (
    <Field label={field.label} required={field.required} error={error}>
      {(id) =>
        field.kind === "long_text" ? (
          <TextArea
            id={id}
            rows={3}
            invalid={Boolean(error)}
            value={text}
            onChange={(e) => onChange(e.currentTarget.value)}
          />
        ) : (
          <TextInput
            id={id}
            type={field.kind === "message_link" ? "url" : "text"}
            invalid={Boolean(error)}
            inputMode={field.kind === "number" ? "decimal" : undefined}
            placeholder={
              field.kind === "message_link"
                ? "https://discord.com/channels/…"
                : field.kind === "number"
                  ? "A number, like 3 or 2.5"
                  : undefined
            }
            value={text}
            onChange={(e) => onChange(e.currentTarget.value)}
          />
        )
      }
    </Field>
  );
}
