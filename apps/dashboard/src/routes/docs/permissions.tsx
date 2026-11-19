import { createFileRoute } from "@tanstack/react-router";

import { Doc, Note, Section, Terms } from "~/features/docs/Doc";

export const Route = createFileRoute("/docs/permissions")({
  component: Permissions,
});

function Permissions() {
  return (
    <Doc
      title="Who can do what"
      lead="Quack has three staff roles: moderators, rules managers, and administrators. Discord permissions and the roles you pick in Quack's settings decide who has which."
    >
      <Section id="roles" title="Who's who">
        <Terms
          items={[
            {
              term: "Moderators",
              meaning:
                "Apply rules, view cases and member history, void cases, edit case context, add evidence, retry, dismiss, and undo actions, review appeals, read the moderation log, and work tickets. The owner, Administrators, and holders of a moderator role are moderators.",
            },
            {
              term: "Rules managers",
              meaning:
                "Create, edit, archive, restore, import, and export rules. The owner, Administrators, members with Manage Server, and holders of a rules manager role are rules managers.",
            },
            {
              term: "Administrators",
              meaning:
                "Change Quack's settings, run /setup, set up the modules, and see server health. The owner, Administrators, and members with Manage Server. Only the owner and Administrators can change the moderator roles.",
            },
            {
              term: "Members",
              meaning:
                "See cases about them and appeal them, by signing in to the dashboard. This works after they leave or are banned.",
            },
          ]}
        />
      </Section>

      <Section id="staff-roles" title="Picking staff roles">
        <p>
          Staff roles live on the dashboard, under <b>Settings</b>. Only the server owner or
          Administrators can change the moderator roles, since a moderator role lets Quack time out,
          kick, and ban for whoever holds it.
        </p>
        <p>
          Until you pick any moderator roles, members with Discord's <b>Timeout Members</b>{" "}
          permission are your moderators. Once you pick one, only those roles count. If every
          moderator role you picked is later deleted in Discord, Timeout Members counts again, so
          nobody is locked out. Rules managers have no such fallback: until you pick a role, only
          members with Manage Server can manage rules.
        </p>
      </Section>

      <Section id="actions" title="Quack takes the action">
        <p>
          Your rules decide the action, and Quack takes it with its own permissions. So a moderator
          role is enough to time out, kick, and ban through rules, even if the moderator's roles
          don't have Timeout Members, Kick Members, or Ban Members in Discord.
        </p>
        <Note tone="warn">
          Only give moderator roles to people you trust with every action your rules can take.
        </Note>
        <p>Quack itself needs the matching permission for each action:</p>
        <ul>
          <li>
            <b>Timeout</b> and <b>remove timeout</b> need Timeout Members.
          </li>
          <li>
            <b>Kick</b> needs Kick Members.
          </li>
          <li>
            <b>Ban</b> and <b>unban</b> need Ban Members.
          </li>
        </ul>
        <p>
          For its channels and modules Quack also uses View Channel, Send Messages, Embed Links,
          Attach Files, Read Message History, Manage Messages (honeypot), Manage Channels (creating
          channels in /setup), Manage Threads, Create Private Threads, and Send Messages in Threads
          (tickets), and View Audit Log (server activity log).
        </p>
        <p>
          If Quack's own access stops a case, it doesn't save the case and records why in the
          moderation log, so an administrator can fix Quack's permissions or role position. The
          dashboard's server health shows what's missing.
        </p>
      </Section>

      <Section id="roles-order" title="Role order matters">
        <p>
          Both the moderator and Quack need a higher role than the member, unless the moderator owns
          the server. Quack also won't add a case about the server owner, a bot, or the moderator
          themselves, and Discord never times out a member with Administrator.
        </p>
        <Note>
          If Quack can't time out or ban someone, the fix is almost always to drag Quack's role
          higher in <b>Server Settings → Roles</b>.
        </Note>
      </Section>

      <Section id="2fa" title="Two-factor authentication">
        <p>
          If your server requires two-factor authentication (2FA) for moderation in Discord, Quack
          requires it for all staff too, the owner included. Quack checks it when someone signs in
          to the dashboard, so staff should turn on 2FA in Discord, then sign in to the dashboard
          once. Members looking at their own cases or tickets don't need it.
        </p>
      </Section>
    </Doc>
  );
}
