import { createFileRoute, Link } from "@tanstack/react-router";

import { Cmd, Doc, Note, Section, Step, Steps } from "~/features/docs/Doc";

export const Route = createFileRoute("/docs/rules")({
  component: Rules,
});

function Rules() {
  return (
    <Doc
      title="Rules"
      lead="A rule is one kind of problem, like spam, and what happens as a member's cases under it add up. Rules are how you tell Quack how your server is moderated."
    >
      <Section id="what" title="What's in a rule">
        <ul>
          <li>
            <b>A name</b>, like Spam, Harassment, or Advertising. Name rules after the problem, not
            the action.
          </li>
          <li>
            <b>A reason</b> the member sees in their DM and on their case. Moderators can't change
            it, so every member gets the same explanation.
          </li>
          <li>
            <b>Levels</b>: what happens at 1 case, and what changes as the case count grows.
          </li>
          <li>
            <b>Whether cases can be appealed.</b>
          </li>
          <li>
            <b>Case context fields</b>, if you want the moderator to fill something in, like what
            the member posted or a message link. Members can see the answers on their case.
          </li>
        </ul>
      </Section>

      <Section id="levels" title="Levels and case counts">
        <p>
          The first level starts at a case count of 1. Later levels start at higher case counts, for
          example: at 1 case, a warning; at 3 cases, a timeout for 1 hour; at 5 cases, a ban. Each
          level can:
        </p>
        <ul>
          <li>take no action, so the case is a warning,</li>
          <li>time the member out for a set length of time, up to 28 days,</li>
          <li>kick them, or</li>
          <li>ban them, optionally deleting their messages from the last 7 days or less.</li>
        </ul>
        <p>
          Quack counts a member's valid cases <b>under that rule only</b>, in that server only,
          including the new one. Three spam cases won't make a harassment case stricter, and voided
          cases don't count. A level keeps applying until the next one starts.
        </p>
        <p>Each level also says whether the member gets a DM about the case.</p>
        <Note>
          Want old cases to stop counting after a while? Set a <b>counting window</b>, like 30 days.
          Older cases stay in the history but no longer raise the level.
        </Note>
      </Section>

      <Section id="make" title="Create or edit a rule">
        <Steps>
          <Step title="Open the dashboard">
            <p>
              Go to the <Link to="/guilds">dashboard</Link>, pick your server, then open{" "}
              <b>Rules</b>.
            </p>
          </Step>
          <Step title="Create a rule or open one">
            <p>Fill in the name and reason, then set up your levels.</p>
          </Step>
          <Step title="Save">
            <p>
              The rule can be applied right away. Editing a rule saves a new version; old cases keep
              the version they used.
            </p>
          </Step>
        </Steps>
        <p>
          Rules are edited on the dashboard. In Discord, <Cmd>/rules</Cmd> lists the active rules
          with a link to each.
        </p>
      </Section>

      <Section id="archive" title="Archiving a rule">
        <p>
          Rules are never deleted. <b>Archive</b> a rule to stop new cases under it. Its cases stay
          in the history and keep counting, and you can restore it any time.
        </p>
        <p>
          You can also export a rule as a rule file and import it in another server to copy your
          setup.
        </p>
      </Section>
    </Doc>
  );
}
