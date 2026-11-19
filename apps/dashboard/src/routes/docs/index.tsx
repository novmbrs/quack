import { createFileRoute, Link } from "@tanstack/react-router";

import { Doc, Note, Section, Terms } from "~/features/docs/Doc";

export const Route = createFileRoute("/docs/")({
  component: Intro,
});

function Intro() {
  return (
    <Doc
      title="What is Quack?"
      lead="Quack is a Discord bot that helps your staff moderate the same way every time. Your server defines the rules, your moderators apply them, and Quack carries out the rule's action and keeps the record."
    >
      <Section id="the-idea" title="The idea">
        <p>
          On most servers, moderators decide on the spot. One moderator gives a warning, another
          gives a ban for the same thing, and nobody remembers who already got a second chance.
        </p>
        <p>
          Quack fixes that with <b>rules</b>. A rule is one kind of problem, like spam, with levels
          that get stricter as a member's case count grows. When a moderator adds a case, they only
          choose the member and the rule. Quack counts the member's cases under that rule, picks the
          level, and carries out its action.
        </p>
      </Section>

      <Section id="words" title="Words you'll see">
        <Terms
          items={[
            {
              term: "Rule",
              meaning:
                "One kind of problem, like spam or harassment, with the levels that say what happens.",
            },
            {
              term: "Level",
              meaning:
                "One rung of a rule. It starts at a case count and can take an action and send a DM. For example: at 1 case, a warning; at 3 cases, a timeout; at 5 cases, a ban.",
            },
            {
              term: "Case count",
              meaning:
                "How many of a member's cases count under one rule, including the new one. Voided cases don't count.",
            },
            {
              term: "Case",
              meaning:
                "The record of a rule applied to a member. It keeps the reason, the case context, any evidence, and what Quack did. A case with no action is a warning.",
            },
            {
              term: "Action",
              meaning:
                "What Quack does in Discord for a case: a timeout, kick, or ban. Remove timeout and unban undo them.",
            },
            {
              term: "Outcome",
              meaning:
                "What actually happened to an action or DM: in progress, done, failed, or couldn't confirm.",
            },
            {
              term: "Void",
              meaning:
                "Keep a case in the history but stop it from counting. Quack also tries to undo its timeout or ban.",
            },
            {
              term: "Appeal",
              meaning: "A member's request to reconsider one of their cases.",
            },
            {
              term: "Moderation log",
              meaning: "Quack's record of who did what. Nothing in it is ever deleted.",
            },
          ]}
        />
      </Section>

      <Section id="where" title="Where you use it">
        <p>
          <b>In Discord</b>, moderators add cases with <b>/case add</b> or by right-clicking a
          message or member. It's the fastest way to deal with something happening right now.
        </p>
        <p>
          <b>On the dashboard</b>, staff edit rules, look up cases, and review appeals. Members sign
          in there too, to see their own cases and appeal them. Both places share the same rules and
          cases.
        </p>
        <Note>
          New here? <Link to="/docs/getting-started">Getting started</Link> takes about five
          minutes.
        </Note>
      </Section>
    </Doc>
  );
}
