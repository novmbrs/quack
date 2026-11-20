//! Slash command, option, and context menu names and descriptions (§11.1,
//! §12.1). `quack-discord` builds the command definitions from these.

/// A command, subcommand, or option: its registered name and description.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Named {
    /// The name Discord registers (lowercase, at most 32 characters).
    pub name: &'static str,
    /// The description, at most 100 characters.
    pub description: &'static str,
}

/// One choice of a string option: the label people see and the value sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Choice {
    /// What people see.
    pub name: &'static str,
    /// What Discord sends back.
    pub value: &'static str,
}

/// `command.case`.
pub const CASE: Named = Named {
    name: "case",
    description: "Add, find, and void moderation cases.",
};
/// `command.case.add`.
pub const CASE_ADD: Named = Named {
    name: "add",
    description: "Add a case by applying a rule to a member.",
};
/// `command.case.add.member`.
pub const CASE_ADD_MEMBER: Named = Named {
    name: "member",
    description: "The member this case is about.",
};
/// `command.case.add.rule`.
pub const CASE_ADD_RULE: Named = Named {
    name: "rule",
    description: "The rule to apply.",
};
/// `command.case.add.private`.
pub const CASE_ADD_PRIVATE: Named = Named {
    name: "private",
    description: "Show the receipt only to you.",
};
/// `command.case.view`.
pub const CASE_VIEW: Named = Named {
    name: "view",
    description: "Open a case by number and see its action and DM outcomes.",
};
/// `command.case.number` (`/case view` and `/case void`).
pub const CASE_NUMBER: Named = Named {
    name: "number",
    description: "The case number shown on its receipt.",
};
/// `command.case.list`.
pub const CASE_LIST: Named = Named {
    name: "list",
    description: "List recent cases.",
};
/// `command.case.list.member`.
pub const CASE_LIST_MEMBER: Named = Named {
    name: "member",
    description: "Only show cases about this member.",
};
/// `command.case.list.rule`.
pub const CASE_LIST_RULE: Named = Named {
    name: "rule",
    description: "Only show cases under this rule.",
};
/// `command.case.void`.
pub const CASE_VOID: Named = Named {
    name: "void",
    description: "Keep a case in the history and stop it from counting.",
};
/// `command.case.void.reason`.
pub const CASE_VOID_REASON: Named = Named {
    name: "reason",
    description: "Why you're voiding the case. The member can see this reason.",
};
/// `command.case.history`.
pub const CASE_HISTORY: Named = Named {
    name: "history",
    description: "Show a member's case history.",
};
/// `command.case.history.member`.
pub const CASE_HISTORY_MEMBER: Named = Named {
    name: "member",
    description: "The member whose cases to show.",
};
/// `command.appeals`.
pub const APPEALS: Named = Named {
    name: "appeals",
    description: "Review appeals waiting for a decision.",
};
/// `command.rules`.
pub const RULES: Named = Named {
    name: "rules",
    description: "List active rules and open them on the dashboard.",
};
/// `command.setup`.
pub const SETUP: Named = Named {
    name: "setup",
    description: "Set up Quack for this server.",
};
/// `command.help`.
pub const HELP: Named = Named {
    name: "help",
    description: "Learn how Quack works.",
};
/// `command.help.topic`.
pub const HELP_TOPIC: Named = Named {
    name: "topic",
    description: "What you want to know about.",
};
/// `command.help.topic.*`: the `/help` topics.
pub const HELP_TOPICS: [Choice; 4] = [
    Choice {
        name: "Cases",
        value: "cases",
    },
    Choice {
        name: "Rules",
        value: "rules",
    },
    Choice {
        name: "Appeals",
        value: "appeals",
    },
    Choice {
        name: "Setup",
        value: "setup",
    },
];
/// `command.add_case`: the message and member command name.
pub const ADD_CASE: &str = "Add case";

/// `command.ops`: the ops server's command (§21.8).
pub const OPS: Named = Named {
    name: "ops",
    description: "See how Quack is running. Only Quack's operators can use this.",
};
/// `command.ops.status`.
pub const OPS_STATUS: Named = Named {
    name: "status",
    description: "Show the status board.",
};
/// `command.ops.server`.
pub const OPS_SERVER: Named = Named {
    name: "server",
    description: "Look up a server Quack knows.",
};
/// `command.ops.server.id`.
pub const OPS_SERVER_ID: Named = Named {
    name: "id",
    description: "The server's ID.",
};
/// `command.ops.jobs`.
pub const OPS_JOBS: Named = Named {
    name: "jobs",
    description: "List dead jobs to requeue or dismiss.",
};
/// Every `/ops` name and description, for limit checks.
pub const OPS_ALL: [Named; 5] = [OPS, OPS_STATUS, OPS_SERVER, OPS_SERVER_ID, OPS_JOBS];

/// Every name and description above, for limit checks.
pub const ALL: [Named; 19] = [
    CASE,
    CASE_ADD,
    CASE_ADD_MEMBER,
    CASE_ADD_RULE,
    CASE_ADD_PRIVATE,
    CASE_VIEW,
    CASE_NUMBER,
    CASE_LIST,
    CASE_LIST_MEMBER,
    CASE_LIST_RULE,
    CASE_VOID,
    CASE_VOID_REASON,
    CASE_HISTORY,
    CASE_HISTORY_MEMBER,
    APPEALS,
    RULES,
    SETUP,
    HELP,
    HELP_TOPIC,
];
