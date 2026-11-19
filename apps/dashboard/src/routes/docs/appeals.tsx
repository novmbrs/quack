import { createFileRoute, Link } from "@tanstack/react-router";

import { Cmd, Doc, Note, Section, Step, Steps } from "~/features/docs/Doc";

export const Route = createFileRoute("/docs/appeals")({
  component: Appeals,
});

function Appeals() {
  return (
    <Doc
      title="Appeals"
      lead="Members can ask the moderators to reconsider a case. Here's how it works on both sides."
    >
      <Section id="members" title="For members">
        <Steps>
          <Step title="Open the DM from Quack">
            <p>
              When Quack adds a case about you, it sends you a DM. If the case can be appealed, the
              DM has an <b>Appeal case</b> button. This works even after a ban.
            </p>
          </Step>
          <Step title="Or sign in to the dashboard">
            <p>
              No DM? Sign in to the <Link to="/cases">dashboard</Link> with Discord and open{" "}
              <b>Your cases</b>. Every case about you is there.
            </p>
          </Step>
          <Step title="Explain why">
            <p>Tell the moderators what you want them to review. Keep it honest and specific.</p>
          </Step>
          <Step title="Wait for a decision">
            <p>
              Quack sends you a DM when the moderators decide or ask for more information. You can
              also check your appeal on your case page and reply there.
            </p>
          </Step>
        </Steps>
        <Note>You can appeal each case once, so take your time with it.</Note>
      </Section>

      <Section id="staff" title="For staff">
        <p>
          Run <Cmd>/setup</Cmd> to choose the channel where new appeals show up. Review them there,
          with <Cmd>/appeals</Cmd>, or on the <b>Appeals</b> page of the dashboard. Every decision
          needs a reason, and the member sees it.
        </p>
        <ul>
          <li>
            <b>Accept</b> voids the case, so it no longer counts. Quack tries to remove its timeout
            or ban; a kick can't be undone. Open the case to check the outcome.
          </li>
          <li>
            <b>Reject</b> keeps the case. It still counts.
          </li>
          <li>
            <b>Request more information</b> asks the member a question. They reply on the dashboard.
          </li>
          <li>
            <b>Close</b> ends the appeal without accepting or rejecting it.
          </li>
          <li>
            <b>Reopen</b> brings a decided or closed appeal back and asks the member for more
            information.
          </li>
        </ul>
        <p>Quack tries to send the member a DM about each decision.</p>
        <Note>
          Set a rejoin invite in <Cmd>/setup</Cmd> under Appeals, so members whose ban is removed
          after an accepted appeal have a way back in.
        </Note>
      </Section>

      <Section id="turn-off" title="Turning appeals off">
        <p>
          Appeals are set per rule. Turn them off for a rule on the dashboard. Cases keep the
          setting of the rule version they used.
        </p>
      </Section>
    </Doc>
  );
}
