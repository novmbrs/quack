import { createFileRoute, Link } from "@tanstack/react-router";
import { ArrowRight, BookOpen } from "lucide-react";

import { CasePreview } from "~/features/site/CasePreview";
import { LadderDemo } from "~/features/site/LadderDemo";
import { SiteFooter, SiteHeader } from "~/features/site/SiteHeader";
import { useTitle } from "~/features/site/useTitle";
import { inviteUrl } from "~/lib/links";
import { ButtonLink, ExternalButton } from "~/ui/Button";
import { QuackIcon, type QuackIconName } from "~/ui/QuackIcon";

import s from "./home.module.css";

export const Route = createFileRoute("/")({
  component: Home,
});

const steps = [
  {
    title: "Write your rules",
    body: "Decide what happens the first time someone breaks a rule, the third time, the fifth. Quack starts you with one you can edit.",
  },
  {
    title: "Add a case",
    body: "A moderator applies a rule to a member, from a slash command, a right-click, or the dashboard.",
  },
  {
    title: "Quack does the rest",
    body: "It counts the member's cases under that rule, picks the level, takes the action, sends the member a DM, and records it in the moderation log.",
  },
];

const features: { icon: QuackIconName; title: string; body: string }[] = [
  {
    icon: "evidence",
    title: "Evidence that sticks",
    body: "Add message links as evidence. Quack saves a copy of each message and its files, so it's still there after the message is deleted.",
  },
  {
    icon: "appeal",
    title: "Fair appeals",
    body: "Members can appeal a case once, from their DM or the dashboard, even after a ban. Accepting an appeal voids the case, and Quack tries to remove its timeout or ban.",
  },
  {
    icon: "history",
    title: "A record you can trust",
    body: "Every case and decision goes in the moderation log. Mistakes are voided, not deleted, so nothing quietly disappears.",
  },
  {
    icon: "search",
    title: "A real dashboard",
    body: "Edit rules, look up a member's case history, and review appeals from your browser.",
  },
  {
    icon: "ticket",
    title: "Support tickets",
    body: "Members open a private thread with your staff. Transcripts are saved when it closes.",
  },
  {
    icon: "shield",
    title: "Spam bot trap",
    body: "A honeypot channel catches bots that post everywhere and adds a case under one of your rules.",
  },
];

function Home() {
  useTitle("Quack · Discord moderation that follows your rules");

  return (
    <div className={s.page}>
      <SiteHeader />
      <main>
        <section className={s.hero}>
          <div className={s.heroText}>
            <h1 className={s.title}>
              Discord moderation
              <br />
              <span className={s.highlight}>that follows your rules</span>
            </h1>
            <p className={s.lead}>
              Set up your server's rules once. Your moderators apply a rule to a member, and Quack
              picks the level, takes the action, and keeps the record. Same rule, same result, every
              time.
            </p>
            <div className={s.ctas}>
              <ExternalButton href={inviteUrl} size="lg" className={s.pill}>
                Add to your server
              </ExternalButton>
              <ButtonLink
                to="/docs"
                variant="secondary"
                size="lg"
                className={s.pill}
                icon={<BookOpen size={17} />}
              >
                How it works
              </ButtonLink>
            </div>
          </div>
          <div className={s.heroArt}>
            <CasePreview />
          </div>
        </section>

        <section className={s.section}>
          <div className={s.sectionHead}>
            <h2 className={s.h2}>Three steps, then it runs itself</h2>
            <p className={s.sub}>
              No more arguing in the staff channel about whether that's a timeout or a ban.
            </p>
          </div>
          <ol className={s.steps}>
            {steps.map((step, i) => (
              <li key={step.title} className={s.step}>
                <span className={s.stepNumber}>{i + 1}</span>
                <h3 className={s.h3}>{step.title}</h3>
                <p className={s.body}>{step.body}</p>
              </li>
            ))}
          </ol>
        </section>

        <section className={s.split}>
          <div className={s.splitText}>
            <h2 className={s.h2}>Escalation, handled</h2>
            <p className={s.sub}>
              Each rule has levels that get stricter as a member's case count grows. Quack counts
              their cases under that rule and picks the level. Your moderators never have to look it
              up.
            </p>
            <p className={s.sub}>
              Every new server starts with this rule. Change its levels, add more rules, or archive
              it. It's your server.
            </p>
            <Link to="/docs/rules" className={s.more}>
              How rules work <ArrowRight size={16} />
            </Link>
          </div>
          <LadderDemo />
        </section>

        <section className={s.section}>
          <div className={s.sectionHead}>
            <h2 className={s.h2}>Everything your staff needs</h2>
            <p className={s.sub}>And nothing it doesn't.</p>
          </div>
          <ul className={s.features}>
            {features.map((f) => (
              <li key={f.title} className={s.feature}>
                <QuackIcon name={f.icon} size={26} />
                <h3 className={s.h3}>{f.title}</h3>
                <p className={s.body}>{f.body}</p>
              </li>
            ))}
          </ul>
        </section>

        <section className={s.final}>
          <h2 className={s.finalTitle}>Give your moderators a break.</h2>
          <p className={s.sub}>Add Quack, check the starter rule, and you're ready.</p>
          <div className={s.ctas}>
            <ExternalButton href={inviteUrl} size="lg" className={s.pill}>
              Add to your server
            </ExternalButton>
            <ButtonLink to="/docs/getting-started" variant="secondary" size="lg" className={s.pill}>
              Setup guide
            </ButtonLink>
          </div>
        </section>
      </main>
      <SiteFooter />
    </div>
  );
}
