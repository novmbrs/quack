import { createFileRoute, Link } from "@tanstack/react-router";

import { Cmd, Doc, Question } from "~/features/docs/Doc";
import { supportUrl } from "~/lib/links";

export const Route = createFileRoute("/docs/faq")({
  component: Faq,
});

function Faq() {
  return (
    <Doc title="Common questions" lead="Quick answers to the things people ask most.">
      <Question q="Where are /warn, /timeout, and /ban?">
        <p>
          There aren't any. Instead of choosing an action, you apply a rule with{" "}
          <Cmd>/case add</Cmd>, and Quack picks the level and its action from the member's case
          count. A warning is a case on a level with no action.
        </p>
      </Question>

      <Question q="Quack couldn't time out or ban someone">
        <p>
          Quack's role needs to be higher than the member's highest role. Go to{" "}
          <b>Server Settings → Roles</b> and drag Quack up. Then open the case with{" "}
          <Cmd>/case view</Cmd> or the dashboard's <b>Failed actions</b> page and retry it.
        </p>
      </Question>

      <Question q="Can I choose a stricter action this one time?">
        <p>
          No, and that's on purpose: the same rule gives the same result for everyone. If something
          deserves a different response, make a separate rule for it, like "Severe harassment" with
          a ban at 1 case.
        </p>
      </Question>

      <Question q="I added a case by mistake">
        <p>
          Void it with <Cmd>/case void</Cmd> or from the case on the dashboard. It stays in the
          history but stops counting, and Quack tries to remove its timeout or ban. See{" "}
          <Link to="/docs/cases" hash="mistakes">
            Fixing a mistake
          </Link>
          .
        </p>
      </Question>

      <Question q="If I edit a rule, do old cases change?">
        <p>
          No. Every case keeps the rule version it used. Edits only affect new cases. Old cases
          still count toward the case count under the edited rule, though.
        </p>
      </Question>

      <Question q="The member didn't get a DM">
        <p>
          Discord doesn't always allow the DM, for example when the member closed their DMs. The
          case still counts, and the case shows whether Quack sent the DM, couldn't send it, or
          couldn't confirm it. The member can see the case by signing in to the dashboard and
          opening Your cases.
        </p>
      </Question>

      <Question q="Does a member's case history carry over between servers?">
        <p>No. Cases only count in the server where they were added.</p>
      </Question>

      <Question q="I got banned. How do I appeal?">
        <p>
          Open the DM Quack sent you and select <b>Appeal case</b>. If you can't find it, sign in to
          the dashboard and open <Link to="/cases">Your cases</Link>. More in{" "}
          <Link to="/docs/appeals">Appeals</Link>.
        </p>
      </Question>

      <Question q="Something else is wrong">
        <p>
          Ask in the <a href={supportUrl}>support server</a> and we'll help you out.
        </p>
      </Question>
    </Doc>
  );
}
