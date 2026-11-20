import { createFileRoute, Link } from "@tanstack/react-router";

import { Cmd, Doc, Note, Section, Step, Steps } from "~/features/docs/Doc";
import { inviteUrl } from "~/lib/links";

export const Route = createFileRoute("/docs/getting-started")({
  component: GettingStarted,
});

function GettingStarted() {
  return (
    <Doc
      title="Getting started"
      lead="Add Quack, give it a moment of setup, and you're ready to moderate."
    >
      <Section id="setup" title="Set up your server">
        <Steps>
          <Step title="Add Quack to your server">
            <p>
              <a href={inviteUrl}>Invite Quack</a> and pick your server. You need the{" "}
              <b>Manage Server</b> permission to add bots.
            </p>
          </Step>
          <Step title="Move Quack's role up">
            <p>
              In <b>Server Settings → Roles</b>, drag the Quack role above the roles of anyone it
              should be able to time out, kick, or ban. Discord won't let a bot act on someone with
              a higher role.
            </p>
          </Step>
          <Step title="Check the starter rule">
            <p>
              Quack comes with one rule, <b>General rule violation</b>: a warning at 1 case, a
              timeout for 1 day at 3 cases, and a ban at 5 cases.
            </p>
            <p>
              Open the <Link to="/guilds">dashboard</Link>, pick your server, and go to <b>Rules</b>{" "}
              to change it or add your own. See <Link to="/docs/rules">Rules</Link> for how.
            </p>
          </Step>
          <Step title="Run /setup">
            <p>
              Run <Cmd>/setup</Cmd> in your server. Pick a log channel for the moderation log and a
              channel where appeals show up, or let Quack create staff-only ones. You can change the
              same settings on the dashboard.
            </p>
          </Step>
          <Step title="Try it out">
            <p>
              Run <Cmd>/case add</Cmd>, pick a member and a rule. Quack posts the case receipt and
              sends the member a DM. Use a friend or an alt account: Quack won't add cases about
              you, bots, or the server owner.
            </p>
          </Step>
        </Steps>
      </Section>

      <Section id="next" title="Optional extras">
        <p>
          When you're ready, <Cmd>/setup</Cmd> also turns these on:
        </p>
        <ul>
          <li>
            <b>Evidence channel</b> keeps copies of evidence files in a staff channel, and lets
            staff upload files as evidence.
          </li>
          <li>
            <b>Tickets</b> let members open private support threads with your staff.
          </li>
          <li>
            <b>Server activity log</b> posts edited and deleted messages, joins, leaves, and other
            server changes to a staff channel.
          </li>
          <li>
            <b>Honeypot</b> sets a trap channel for spam bots.
          </li>
        </ul>
        <p>
          More on each in <Link to="/docs/extras">Tickets, logs & more</Link>.
        </p>
        <Note>
          Stuck? Run <Cmd>/help</Cmd> in your server for a quick overview, with a link to the
          dashboard.
        </Note>
      </Section>
    </Doc>
  );
}
