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
            "Quack keeps track of moderation cases. Rules managers define the rules on the dashboard, moderators apply them, and Quack carries out the rule's action.",
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
                .s(". Choose a rule. Quack counts the member's valid cases under that rule, including the new case, and uses that count to pick the level and carry out its action. Set private to True to show the receipt only to you.")),
            p(Text::fixed("When you add a case from a message, Quack saves that message as evidence. Add more with ")
                .bold("Add evidence")
                .s(" on the case. Members can see the case context and evidence on their case page; Quack hides staff names and message authors there.")),
            p(Text::fixed("Made a mistake? Use ")
                .command(Command::CaseVoid)
                .s(" or ")
                .bold("Void case")
                .s(". The case stays in the history but stops counting. Open the case afterwards to check whether Quack removed its timeout or ban; a kick can't be undone.")),
            p(Text::fixed(
                "If an action fails, open the case to see which actions you can retry or dismiss. Open Failed actions on the dashboard to review every outstanding failure.",
            )),
        ],
        Topic::Rules => vec![
            Block::Heading(Text::fixed("Rules")),
            p(Text::fixed(
                "A rule is one kind of problem, like spam. Each rule has levels that start at a case count, for example: at 1 case, a warning; at 3 cases, a 1-hour timeout; at 5 cases, a ban. Quack counts each member's valid cases under the rule, including the new one; voided cases don't count. A rule can count only recent cases, like the last 30 days.",
            )),
            p(Text::fixed("Rules are edited on the dashboard, and editing a rule never changes old cases. Use ")
                .command(Command::Rules)
                .s(" to see this server's rules.")),
        ],
        Topic::Appeals => vec![
            Block::Heading(Text::fixed("Appeals")),
            p(Text::fixed("Members can appeal eligible cases once, from Quack's DM or the dashboard, even after a ban. Use ")
                .command(Command::Setup)
                .s(" to choose where new appeals appear. Moderators can also use ")
                .command(Command::Appeals)
                .s(" or the dashboard. Accepting an appeal voids the case so it stops counting. Open the case to check whether its timeout or ban was removed.")),
        ],
        Topic::Setup => vec![
            Block::Heading(Text::fixed("Setup")),
            p(Text::fixed("Use ")
                .command(Command::Setup)
                .s(" to pick channels or let Quack create them, and to turn on tickets, the honeypot, and the server activity log. Keep the log, appeal, evidence, ticket staff, and server activity log channels staff-only. Members need access to the ticket entry and honeypot channels. You can also change these settings on the dashboard.")),
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
