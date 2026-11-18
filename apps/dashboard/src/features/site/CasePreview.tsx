import { Hash } from "lucide-react";

import { Logo } from "~/ui/Logo";
import { QuackIcon } from "~/ui/QuackIcon";

import s from "./CasePreview.module.css";

/**
 * CasePreview is the homepage's picture of Quack at work: a moderator's
 * /case add and the receipt Quack posts, with the member's DM beside it. The
 * copy follows the bot's messages (design §12.4 and §12.8).
 */
export function CasePreview() {
  return (
    <figure className={s.preview} aria-label="Example of Quack in Discord">
      <div className={s.window}>
        <div className={s.bar}>
          <Hash size={16} />
          <span>mod-chat</span>
        </div>
        <div className={s.body}>
          <div className={s.used}>
            <span className={s.reply} aria-hidden />
            <span className={s.mini} data-tone="mira" aria-hidden>
              M
            </span>
            <span className={s.who}>Mira</span> used <span className={s.slash}>/case add</span>
          </div>
          <div className={s.message}>
            <Logo size={40} />
            <div className={s.content}>
              <div className={s.meta}>
                <span className={s.name}>Quack</span>
                <span className={s.app}>APP</span>
                <span className={s.time}>Today at 4:20 PM</span>
              </div>
              <p className={s.line}>
                <QuackIcon name="case_add" size={18} />
                <span>
                  <b>Case #42 · Spam</b>
                </span>
              </p>
              <p className={s.sub}>
                <span className={s.mention}>@loud_larry</span>
              </p>
              <p className={s.sub}>Posting spam or unsolicited advertising.</p>
              <p className={s.sub}>
                <b>Action:</b> Quack set <span className={s.mention}>@loud_larry</span>'s timeout to
                end tomorrow at 4:20 PM.
              </p>
            </div>
          </div>
        </div>
      </div>

      <div className={s.dm}>
        <div className={s.dmHead}>
          <Logo size={22} />
          <span className={s.name}>Quack</span>
          <span className={s.dmTag}>in your DMs</span>
        </div>
        <p className={s.line}>
          <QuackIcon name="timeout" size={18} />
          <span>
            <b>Case #42 in The Pond</b> · Rule: Spam. The rule calls for a timeout until tomorrow at
            4:20 PM.
          </span>
        </p>
        <span className={s.appeal}>
          <QuackIcon name="appeal" size={16} />
          Appeal case
        </span>
      </div>
    </figure>
  );
}
