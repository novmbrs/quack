import { createFileRoute, Link } from "@tanstack/react-router";

import { CommandTable, Doc, Note, Section } from "~/features/docs/Doc";

export const Route = createFileRoute("/docs/commands")({
  component: Commands,
});

function Commands() {
  return (
    <Doc
      title="Commands"
      lead="Every Quack command in one place. You'll mostly use /case add; the rest are there when you need them."
    >
      <Section id="cases" title="Cases">
        <CommandTable
          rows={[
            ["/case add", "Add a case by applying a rule to a member."],
            [
              "Apps → Add case (on a message)",
              "Add a case about the message's author. Quack saves the message as evidence.",
            ],
            ["Apps → Add case (on a member)", "Add a case about that member."],
            ["/case view", "Open a case by number and see its action and DM outcomes."],
            ["/case list", "List recent cases, optionally about one member or under one rule."],
            ["/case history", "Show a member's case history."],
            ["/case void", "Keep a case in the history and stop it from counting."],
          ]}
        />
        <p>
          The case card from <b>/case view</b> has buttons to retry or dismiss a failed action,
          remove a timeout, unban, edit the case context, add evidence, and void the case.
        </p>
      </Section>

      <Section id="appeals" title="Appeals">
        <CommandTable rows={[["/appeals", "Review appeals waiting for a decision."]]} />
      </Section>

      <Section id="rules" title="Rules">
        <CommandTable rows={[["/rules", "List active rules and open them on the dashboard."]]} />
        <p>
          Rules are edited on the <Link to="/guilds">dashboard</Link>. See{" "}
          <Link to="/docs/rules">Rules</Link>.
        </p>
      </Section>

      <Section id="setup" title="Setup and help">
        <CommandTable
          rows={[
            [
              "/setup",
              "Set up Quack for this server: the log channel, appeals, evidence copies, tickets, the honeypot, and the server activity log. Needs Manage Server.",
            ],
            ["/help", "Learn how Quack works, with topics for cases, rules, appeals, and setup."],
          ]}
        />
        <Note>
          Quack's replies to staff are private: only you see them. The one exception is the case
          receipt from <b>/case add</b>, which Quack posts in the channel unless you set{" "}
          <b>private</b> to True.
        </Note>
      </Section>
    </Doc>
  );
}
