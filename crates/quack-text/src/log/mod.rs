//! Moderation log entries (§12.10): the log channel post for each entry,
//! and the same sentence as the dashboard's `text`.
//!
//! Entries are rendered from `quack_core::modlog::ModLogEntry` (the typed
//! detail) plus [`LogFacts`], the display names and links the poster reads
//! when it renders. Adding an entry kind fails to compile here until it has
//! words.

mod actions;
mod kinds;

use quack_core::action::ActionKind;
use quack_core::ids::ActionId;
use quack_core::modlog::{ModLogEntry, ModLogResult};
use quack_core::modules::ModuleName;
use time::OffsetDateTime;

use crate::actions::ButtonAction;
use crate::model::{
    Actor, Block, Button, ButtonStyle, ChannelRef, Inline, Msg, Person, Text, TimeStyle, Tone,
};

pub use kinds::{lead, quote};

/// The names and links an entry's sentence needs besides its detail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogFacts {
    /// When the entry was written.
    pub at: OffsetDateTime,
    /// Who acted; Quack when the entry has no actor.
    pub actor: Actor,
    /// The member the entry is about.
    pub member: Option<Person>,
    /// The case's number, for entries whose detail doesn't carry it (DMs).
    pub case_number: Option<u64>,
    /// The case on the dashboard; log channel posts link `{case}` to it.
    pub case_url: Option<String>,
    /// The rule's name.
    pub rule: Option<String>,
    /// The void reason of a voided case.
    pub void_reason: Option<String>,
    /// The timeout's fixed end, for a done timeout.
    pub timeout_ends_at: Option<OffsetDateTime>,
    /// The ticket's thread.
    pub thread: Option<ChannelRef>,
    /// The entry's page on the dashboard, which the post links to under
    /// that page's name.
    pub dashboard: Option<LogLink>,
    /// For a retryable failed action: the action the Retry button retries.
    pub retry_action: Option<ActionId>,
    /// The level's action, for refusals that name it (a missing permission
    /// on a honeypot case or a refused case).
    pub action: Option<ActionKind>,
}

impl LogFacts {
    /// Facts with only a time and an actor.
    pub const fn new(at: OffsetDateTime, actor: Actor) -> Self {
        Self {
            at,
            actor,
            member: None,
            case_number: None,
            case_url: None,
            rule: None,
            void_reason: None,
            timeout_ends_at: None,
            thread: None,
            dashboard: None,
            retry_action: None,
            action: None,
        }
    }

    pub(crate) fn member(&self, t: Text) -> Text {
        match &self.member {
            Some(m) => t.person(m),
            None if t.is_empty() => t.s("A member"),
            None => t.s("a member"),
        }
    }

    /// `{case}`: a bold link to the case in Discord posts, "case #N" text
    /// elsewhere.
    pub(crate) fn case(&self, t: Text, number: u64) -> Text {
        let label = format!("case #{number}");
        match &self.case_url {
            Some(url) => t.push(Inline::Bold(vec![Inline::Link {
                text: vec![Inline::Fixed(label)],
                url: url.clone(),
            }])),
            None => t.s(label),
        }
    }

    pub(crate) fn rule(&self, t: Text) -> Text {
        match &self.rule {
            Some(r) => t.bold_user(r),
            None => t.s("a rule"),
        }
    }
}

/// A dashboard page a log post links to (§14.11).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogPage {
    /// The entry's case.
    Case,
    /// The entry's appeal.
    Appeal,
    /// The failed actions list.
    FailedActions,
    /// The entry's rule.
    Rule,
    /// The server's settings.
    Settings,
    /// The moderation log itself.
    ModerationLog,
}

impl LogPage {
    /// The link's label: where it goes.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Case => "Open case",
            Self::Appeal => "Open appeal",
            Self::FailedActions => "Open failed actions",
            Self::Rule => "Open rule",
            Self::Settings => "Open settings",
            Self::ModerationLog => "Open moderation log",
        }
    }
}

/// A log post's dashboard link: the page it goes to, and its URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogLink {
    /// Which page.
    pub page: LogPage,
    /// Its URL.
    pub url: String,
}

/// The entry as one paragraph of text (the dashboard's `text`): the lead
/// sentence, and its quote on a new line.
pub fn text(entry: &ModLogEntry, f: &LogFacts) -> Text {
    let lead = lead(entry, f);
    match quote(entry, f) {
        Some(q) => lead.s("\n> ").then(q),
        None => lead,
    }
}

/// The log channel post: the lead sentence, an optional quote, and
/// `-# {time} · [Open case](<link>)`, the link named for the page it opens
/// (case, appeal, failed actions, rule, settings, or moderation log); a
/// **Retry** button on a retryable failed action.
pub fn post(entry: &ModLogEntry, f: &LogFacts) -> Msg {
    let tone = match entry.result() {
        Some(ModLogResult::Ok) | None => Tone::Neutral,
        Some(ModLogResult::Failed | ModLogResult::Refused) => Tone::Danger,
    };
    let mut msg = Msg::new(tone).para(lead(entry, f));
    if let Some(q) = quote(entry, f) {
        msg = msg.block(Block::Quote(q));
    }
    let mut sub = Text::new().time(f.at, TimeStyle::Full);
    if let Some(link) = &f.dashboard {
        sub = sub.s(" · ").link(link.page.label(), &link.url);
    }
    msg = msg.block(Block::Subtext(sub));
    if let (ModLogEntry::ActionFailed(d), Some(id)) = (entry, f.retry_action) {
        msg = msg.buttons(vec![Button::new(
            crate::word::retry_label(d.kind),
            ButtonStyle::Primary,
            ButtonAction::RetryAction(id),
        )]);
    }
    msg
}

/// `setting.*`: a server setting's name in `{fields}`, by its key or API
/// field name. Anything else reads as "another setting", so a machine name
/// never reaches a sentence.
pub fn setting_name(key: &str) -> &'static str {
    match key {
        "log_channel" | "log_channel_id" => "log channel",
        "appeal_channel" | "appeal_channel_id" => "appeal channel",
        "evidence_channel" | "evidence_channel_id" => "evidence channel",
        "appeal_invite" | "appeal_invite_url" => "rejoin invite",
        "dm_intro" => "DM introduction",
        "dm_footer" => "DM footer",
        "moderator_roles" | "moderator_role_ids" => "moderator roles",
        "rules_manager_roles" | "rules_manager_role_ids" => "rules manager roles",
        "starter_notice" | "starter_notice_acknowledged_at" => "starter rule reminder",
        _ => "another setting",
    }
}

/// `setting.*`: a module setting's name in `{fields}`, by its key or API
/// field name within that module's settings.
pub fn module_setting_name(module: ModuleName, key: &str) -> &'static str {
    match (module, key) {
        (ModuleName::Tickets, "entry_channel" | "entry_channel_id" | "ticket_entry_channel") => {
            "ticket entry channel"
        }
        (ModuleName::Tickets, "queue_channel" | "queue_channel_id" | "ticket_queue_channel") => {
            "ticket staff channel"
        }
        (ModuleName::Tickets, "transcript_retention" | "transcript_retention_days") => {
            "transcript retention"
        }
        (ModuleName::Honeypot, "channel" | "channel_id" | "honeypot_channel") => "honeypot channel",
        (ModuleName::Honeypot, "rule" | "rule_id" | "honeypot_rule") => "honeypot rule",
        (ModuleName::Honeypot, "warning" | "warning_text" | "honeypot_warning") => {
            "honeypot warning"
        }
        (ModuleName::ActivityLog, "routes" | "activity_routes") => "server activity channels",
        (ModuleName::ActivityLog, "include_message_content" | "activity_content") => {
            "message text in the server activity log"
        }
        (ModuleName::Tickets | ModuleName::Honeypot | ModuleName::ActivityLog, _) => {
            "another setting"
        }
    }
}

/// `{fields}`: setting names joined with commas and "and", each once.
pub fn fields(names: impl IntoIterator<Item = &'static str>) -> String {
    let mut unique: Vec<String> = Vec::new();
    for name in names {
        if !unique.iter().any(|n| n == name) {
            unique.push(name.to_owned());
        }
    }
    crate::fmt::list(&unique, "and")
}
