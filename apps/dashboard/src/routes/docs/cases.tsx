import { createFileRoute, Link } from "@tanstack/react-router";

import { Cmd, Doc, Note, Section } from "~/features/docs/Doc";

export const Route = createFileRoute("/docs/cases")({
  component: Cases,
});

function Cases() {
  return (
    <Doc
      title="Cases"
      lead="A case is the record of a rule applied to a member. Adding one is how you moderate with Quack."
    >
      <Section id="add" title="Add a case">
        <p>There are three ways, and they all do the same thing:</p>
        <ul>
          <li>
            Run <Cmd>/case add</Cmd>, then choose the member and the rule.
          </li>
          <li>
            Right-click a message and choose <Cmd>Apps → Add case</Cmd>. Quack saves the message as
            evidence.
          </li>
          <li>
            Right-click a member and choose <Cmd>Apps → Add case</Cmd>.
          </li>
        </ul>
        <p>
          You can also add cases on the dashboard, on the <b>Cases</b> page, where Quack shows which
          level the rule will use before you save.
        </p>
      </Section>

      <Section id="what-happens" title="What happens next">
        <p>Quack takes it from there:</p>
        <ul>
          <li>
            Counts the member's valid cases under that rule, including the new one, and picks the
            level.
          </li>
          <li>Times out, kicks, or bans the member if that level has an action.</li>
          <li>Sends the member one DM about the case, if the level sends one.</li>
          <li>
            Posts the case receipt in the channel. Set <b>private</b> to True to show it only to
            you.
          </li>
          <li>Records it all in the moderation log.</li>
        </ul>
        <p>
          Open the case with <Cmd>/case view</Cmd> or on the dashboard to see each outcome. An
          action or DM is in progress, done, failed, or Quack couldn't confirm it. For the DM, that
          reads sent, couldn't send, couldn't confirm, or not needed. When Quack couldn't send a DM,
          the member can still see the case by signing in to the dashboard.
        </p>
        <Note>
          Quack takes the action with its own permissions, so you don't need Ban Members for a case
          that bans. You do need a higher role than the member. See{" "}
          <Link to="/docs/permissions">Who can do what</Link>.
        </Note>
      </Section>

      <Section id="evidence" title="Evidence">
        <p>
          Evidence is saved message snapshots and their files. Add case from a message, message
          links in the case context, and <b>Add evidence</b> on the case all add evidence. Quack
          saves a copy of each message. With evidence copies turned on in <Cmd>/setup</Cmd>, it also
          keeps copies of the files in a staff channel, so they're still there if the original gets
          deleted. Members can see the evidence on their case, but not who wrote it.
        </p>
        <Note tone="warn">
          Quack can't save a message that's already gone. Add the case before deleting the message.
        </Note>
      </Section>

      <Section id="mistakes" title="Fixing a mistake">
        <p>
          A case's decision never changes. If a case was wrong, <b>void</b> it with{" "}
          <Cmd>/case void</Cmd> or <b>Void case</b>. A voided case:
        </p>
        <ul>
          <li>stays in the history, marked as voided,</li>
          <li>no longer counts toward the member's case count, and</li>
          <li>
            has its timeout or ban removed when Quack can. A kick can't be undone. Open the case
            afterwards to check the outcome.
          </li>
        </ul>
        <p>
          Then add a new case with the right rule if you need to. On the dashboard you can link it
          as the replacement. You can still edit a case's context and add evidence after it's saved.
        </p>
      </Section>

      <Section id="look-up" title="Looking things up">
        <ul>
          <li>
            <Cmd>/case list</Cmd> lists recent cases.
          </li>
          <li>
            <Cmd>/case history</Cmd> shows one member's case history.
          </li>
          <li>
            <Cmd>/case view</Cmd> opens one case by number, with buttons to retry or dismiss a
            failed action.
          </li>
        </ul>
        <p>
          The dashboard has all of this too, with filters, and a <b>Failed actions</b> page for
          actions that didn't go through, usually because Quack's role was too low.
        </p>
      </Section>
    </Doc>
  );
}
