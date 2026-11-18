//! `/help` (§12.12): static help text, one message per topic.

use crate::model::{Block, Button, Command, Msg, Text, Tone};

/// A `/help` topic (`command.help.topic.*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Topic {
    /// Cases.
    Cases,
    /// Rules.
    Rules,
    /// Appeals.
    Appeals,
    /// Setup.
    Setup,
}

fn commands(t: Text, list: &[Command]) -> Text {
    let mut t = t;
    for (i, c) in list.iter().enumerate() {
        if i > 0 {
            t = t.s(", ");
        }
        t = t.command(*c);
    }
    t
}

fn overview() -> Vec<Block> {
    vec![
        Block::Heading(Text::fixed("Quack")),
        Block::Text(Text::fixed(
            "Quack keeps track of moderation cases. Rules managers set up rules on the dashboard, moderators apply them, and Quack picks the outcome and carries it out.",
        )),
        Block::Text(commands(
            Text::new().bold("Moderators:").s(" "),
            &[
                Command::CaseAdd,
                Command::CaseView,
                Command::CaseList,
                Command::CaseHistory,
                Command::CaseVoid,
                Command::Appeals,
            ],
        )),
        Block::Text(
            Text::new()
                .bold("Rules:")
                .s(" ")
                .command(Command::Rules)
                .s(" (rules are edited on the dashboard)"),
        ),
        Block::Text(
            Text::new()
                .bold("Setup:")
                .s(" ")
                .command(Command::Setup)
                .s(" (needs Manage Server)"),
        ),
        Block::Text(
            Text::fixed("Use ")
                .command(Command::Help)
                .s(" with a topic for more."),
        ),
    ]
}

fn topic(t: Topic) -> Vec<Block> {
    let p = |t: Text| Block::Text(t);
    match t {
        Topic::Cases => vec![
            Block::Heading(Text::fixed("Cases")),
            p(Text::fixed("Use ")
                .command(Command::CaseAdd)
                .s(", or right-click a message or member and choose ")
                .bold("Apps → Add case")
                .s(". Choose the rule, and Quack counts the member's earlier cases under it, picks the level, and carries out its action. Set private to True to keep the receipt to yourself.")),
            p(Text::fixed("When you add a case from a message, Quack saves that message as evidence. Add more with ")
                .bold("Add evidence")
                .s(" on the case. The member can see the case context, but not who added the case or who wrote the evidence.")),
            p(Text::fixed("Made a mistake? Use ")
                .command(Command::CaseVoid)
                .s(" or ")
                .bold("Void case")
                .s(". The case stays in the history but stops counting. Open the case afterwards to check whether Quack removed its timeout or ban; a kick can't be undone.")),
            p(Text::fixed("If Quack couldn't carry out an action, the case shows ")
                .bold("Retry")
                .s(" and ")
                .bold("Dismiss")
                .s(", and the dashboard's failed actions page lists every one.")),
        ],
        Topic::Rules => vec![
            Block::Heading(Text::fixed("Rules")),
            p(Text::fixed(
                "A rule is one kind of problem, like spam. Each rule has levels that start at a case count: for example a warning for the first case, a 1-hour timeout from the third, and a ban from the fifth. Counts are per member and per rule, and voided cases don't count. A rule can count only recent cases, like the last 30 days.",
            )),
            p(Text::fixed("Rules are edited on the dashboard, and editing a rule never changes old cases. Use ")
                .command(Command::Rules)
                .s(" to see this server's rules.")),
        ],
        Topic::Appeals => vec![
            Block::Heading(Text::fixed("Appeals")),
            p(Text::fixed("A member can appeal each case once, from Quack's DM or from the dashboard, even after a ban. Choose the appeal channel with ")
                .command(Command::Setup)
                .s(". Moderators accept or reject appeals there, with ")
                .command(Command::Appeals)
                .s(", or on the dashboard, and Quack tries to send the member a DM with the decision. Accepting an appeal voids the case, and Quack removes the timeout or ban it placed when Discord still shows it.")),
        ],
        Topic::Setup => vec![
            Block::Heading(Text::fixed("Setup")),
            p(Text::fixed("Run ")
                .command(Command::Setup)
                .s(" to choose the log channel, the appeal channel, and the evidence channel, and to turn on tickets, the honeypot, and the server activity log. Quack can create each channel for you with staff-only permissions. Everything in ")
                .command(Command::Setup)
                .s(" is also in the dashboard settings.")),
        ],
    }
}

/// `help.*`: the overview without a topic, or one topic, with
/// `help.dashboard_link`.
pub fn help(t: Option<Topic>, dashboard_url: &str) -> Msg {
    let mut msg = Msg::new(Tone::Neutral);
    msg.blocks = t.map_or_else(overview, topic);
    msg.buttons(vec![Button::link("Open the dashboard", dashboard_url)])
}
