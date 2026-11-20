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
            [
              "/case add",
              "Add a case by applying a rule to a member. Attach a file to upload it as evidence with the case.",
            ],
            [
              "Apps → Add case (on a message)",
              "Add a case about the message's author. Quack saves the message as evidence.",
            ],
            ["Apps → Add case (on a member)", "Add a case about that member."],
            [
              "/case evidence",
              "Add evidence to a case by number: message links, up to three files, or both, with an optional note.",
            ],
            [
              "Apps → Add as evidence (on a message)",
              "Save the message as evidence on one of the author's cases, one you added recently, or a case number you enter.",
            ],
            ["/case view", "Open a case by number and see its action and DM outcomes."],
            ["/case list", "List recent cases, optionally about one member or under one rule."],
            ["/case history", "Show a member's case history."],
            ["/case void", "Keep a case in the history and stop it from counting."],
          ]}
        />
        <p>
          The case card from <b>/case view</b> has buttons to retry or dismiss a failed action,
          remove a timeout, unban, edit the case context, add or view evidence, and void the case.
          After <b>/case add</b>, the private confirmation has <b>Add evidence</b> and{" "}
          <b>View evidence</b> too, so you can add links and files right away.
        </p>
        <p>
          <b>Add evidence</b> opens a form for message links, up to 10 files, and a staff note.{" "}
          <b>View evidence</b> shows one item at a time, with buttons to edit its note, see the
          messages around a saved message, show uploaded files to the member or hide them, and
          remove it. Images and videos show as spoilers.
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
              "Set up Quack for this server: the log channel, appeals, the evidence channel, tickets, the honeypot, and the server activity log. Needs Manage Server.",
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
