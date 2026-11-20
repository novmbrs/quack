import { createFileRoute } from "@tanstack/react-router";

import { Cmd, Doc, Note, Section } from "~/features/docs/Doc";

export const Route = createFileRoute("/docs/extras")({
  component: Extras,
});

function Extras() {
  return (
    <Doc
      title="Tickets, logs & more"
      lead="Channels and optional modules you set up with /setup or on the dashboard. None of the modules are needed to use Quack."
    >
      <Note>
        In <Cmd>/setup</Cmd>, choose a channel you already have or let Quack create one. Keep the
        log, appeal, evidence, ticket staff, and server activity log channels staff-only. Members
        need access to the ticket entry and honeypot channels.
      </Note>

      <Section id="log-channel" title="Log channel">
        <p>
          Quack posts each moderation log entry in the log channel: new cases, voids, failed
          actions, appeal decisions, rule changes, and so on. The full moderation log is always on
          the dashboard too.
        </p>
      </Section>

      <Section id="evidence" title="Evidence channel">
        <p>
          Pick a staff-only channel where Quack keeps copies of evidence files, so they stay
          viewable after the original message is deleted. Files that staff upload as evidence are
          kept only there, so uploading needs it: without one, Quack refuses uploads and keeps each
          saved message's file details and a link to the original. Quack checks the channel is still
          staff-only before every copy.
        </p>
      </Section>

      <Section id="tickets" title="Tickets">
        <p>
          Members open a private thread with your staff from a panel in the entry channel, and staff
          get a post about each new ticket in a staff channel. When a ticket closes, Quack saves a
          transcript and tries to send the member a DM with it. Transcripts stay on the dashboard
          for 90 days by default.
        </p>
      </Section>

      <Section id="activity-log" title="Server activity log">
        <p>
          Posts Discord events to staff-only channels: edited and deleted messages, members joining,
          leaving, banned, and unbanned, and changes to channels and the server. Route every event
          to one channel, or each to its own on the dashboard. It isn't a searchable archive.
        </p>
        <Note>To show who banned or unbanned a member, Quack's role needs View Audit Log.</Note>
      </Section>

      <Section id="honeypot" title="Honeypot">
        <p>
          A trap channel with a warning not to post in it. Real members read the warning. Spam bots
          don't. When someone who isn't staff posts there, Quack adds a case under the rule you
          choose and deletes the post.
        </p>
        <Note tone="warn">
          Check which rule the honeypot uses before turning it on. Whatever that rule's levels do,
          Quack does automatically. The rule can't have required case context.
        </Note>
      </Section>

      <Section id="pause" title="Turning a module off">
        <p>
          Turn tickets, the server activity log, or the honeypot on and off in <Cmd>/setup</Cmd> or
          on the module's dashboard page. Your settings are kept. If a module's channel is deleted,
          Quack turns the module off and the dashboard says what to fix.
        </p>
      </Section>
    </Doc>
  );
}
