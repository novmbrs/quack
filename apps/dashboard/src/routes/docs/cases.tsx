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
        <p>Evidence is what shows what happened. Each evidence item is one of two kinds:</p>
        <ul>
          <li>
            A <b>saved message</b>: Quack's copy of a Discord message, its files, and up to 5
            messages before and after it.
          </li>
          <li>
            <b>Uploaded files</b>: up to 10 files a moderator uploaded, such as a screenshot from a
            DM. Quack saves images, videos, audio, plain text, and PDFs up to 10 MB each.
          </li>
        </ul>
        <p>Ways to add evidence:</p>
        <ul>
          <li>
            Right-click a message and choose <Cmd>Apps → Add case</Cmd> or{" "}
            <Cmd>Apps → Add as evidence</Cmd>. Add as evidence offers the author's cases and the
            ones you added recently, or a case number.
          </li>
          <li>
            Put message links in the case context, or give <Cmd>/case add</Cmd> a <b>file</b>.
          </li>
          <li>
            Run <Cmd>/case evidence</Cmd> with a case number and message links, up to three files,
            or both.
          </li>
          <li>
            Select <b>Add evidence</b> on a case or after adding one: paste message links, attach
            files, or both.
          </li>
          <li>
            On the dashboard's case page, drop files on <b>Evidence</b>, paste them, or select{" "}
            <b>Upload files</b>. <b>Add message links</b> saves messages.
          </li>
        </ul>
        <p>
          Every item can have a short <b>staff note</b>, such as where it came from. Only staff see
          notes. Add one while adding evidence, or edit it later.
        </p>
        <Note>
          Uploaded files are kept as copies in the server's evidence channel, so set a staff-only
          one in <Cmd>/setup</Cmd> first. Quack won't upload anything without it. With an evidence
          channel set, Quack also copies the files of saved messages there, so they're still there
          if the original gets deleted.
        </Note>
        <Note tone="warn">
          Quack can't save a message that's already gone. Add the case before deleting the message.
        </Note>
      </Section>

      <Section id="evidence-members" title="What the member sees">
        <p>The member can open their own case on the dashboard. There they see:</p>
        <ul>
          <li>Saved messages: the text and files, never who sent them or where.</li>
          <li>
            Uploaded files only once staff select <b>Show to member</b>. New uploads are staff only,
            and <b>Hide from member</b> hides them again.
          </li>
        </ul>
        <p>
          The member never sees staff notes, the messages around a saved message, or removed
          evidence.
        </p>
      </Section>

      <Section id="evidence-remove" title="Removing evidence">
        <p>
          If evidence was added by mistake, select <b>Remove</b> on it. Discord and the member stop
          seeing it, and the case page lists it collapsed as removed. Nothing is deleted: the
          moderation log records who removed it, and file copies stay in the evidence channel.
          Adding the same message link again brings a removed message back. Removed uploads don't
          come back; upload the files again.
        </p>
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
