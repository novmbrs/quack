import { Fragment } from "react";

import { QuackMessage } from "~/features/discord/QuackMessage";

import type { CaseDm, Segment } from "./caseDm";
import s from "./DmPreview.module.css";

/**
 * DmPreview draws a case DM the way the member sees it in Discord: the
 * heading, the rule and date, the quoted reason, the paragraphs, and the
 * buttons.
 */
export function DmPreview({ dm }: { dm: CaseDm }) {
  return (
    <QuackMessage label="Preview of the member's DM" buttons={dm.buttons}>
      <div className={s.paragraphs}>
        <p className={s.heading}>{dm.heading}</p>
        <p className={s.line}>
          <Segments segments={dm.rule} />
        </p>
        {dm.quote.trim() ? <blockquote className={s.quote}>{dm.quote}</blockquote> : null}
        {dm.paragraphs.map((segments, i) => (
          <p key={i} className={s.line}>
            <Segments segments={segments} />
          </p>
        ))}
      </div>
    </QuackMessage>
  );
}

function Segments({ segments }: { segments: Segment[] }) {
  return (
    <>
      {segments.map((seg, i) => (
        <Fragment key={i}>
          {seg.strong ? (
            <strong className={s.strong}>{seg.text}</strong>
          ) : seg.time ? (
            <span className={s.timestamp}>{seg.text}</span>
          ) : (
            seg.text
          )}
        </Fragment>
      ))}
    </>
  );
}
