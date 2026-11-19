//! The honeypot warning (§12.13, §13.4.3).

use quack_core::rule::LevelAction;

use crate::fmt;
use crate::model::{Block, Inline, Msg, Text, Tone};

/// `honeypot.outcome.*`: what Quack does to someone who posts.
pub fn outcome(action: Option<LevelAction>) -> String {
    match action {
        None => "give you a warning".to_owned(),
        Some(LevelAction::Timeout { duration_seconds }) => {
            format!(
                "time you out for {}",
                fmt::duration(u64::from(duration_seconds))
            )
        }
        Some(LevelAction::Kick) => "remove you from the server".to_owned(),
        Some(LevelAction::Ban { .. }) => "ban you".to_owned(),
    }
}

/// What the warning shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Warning {
    /// The honeypot is off.
    Off,
    /// The honeypot is on.
    On {
        /// The rule's name.
        rule: String,
        /// The rule's levels' actions, in order.
        levels: Vec<Option<LevelAction>>,
        /// The server's own warning, replacing the generated one.
        custom: Option<String>,
        /// Distinct cases created from honeypot posts.
        cases: u64,
    },
}

/// `honeypot.warning.*`: the warning message in the honeypot channel.
pub fn warning(w: &Warning) -> Msg {
    let Warning::On {
        rule,
        levels,
        custom,
        cases,
    } = w
    else {
        return Msg::new(Tone::Neutral)
            .block(Block::Heading(Text::fixed("The honeypot is off")))
            .para(Text::fixed("Posts here don't add cases right now."));
    };
    let mut msg = Msg::new(Tone::Danger);
    if let Some(text) = custom.as_deref().filter(|c| !c.trim().is_empty()) {
        msg = msg.para(Text::new().push(Inline::ServerMarkdown(text.to_owned())));
    } else {
        {
            let mut outcomes: Vec<String> = Vec::new();
            for o in levels.iter().map(|a| outcome(*a)) {
                if !outcomes.contains(&o) {
                    outcomes.push(o);
                }
            }
            let lead = Text::fixed("If you post here, Quack can add a case under ")
                .bold_user(rule)
                .s(".");
            let sentence = match outcomes.as_slice() {
                [] => lead,
                [one] => lead.s(format!(" This rule can {one}.")),
                many => lead.s(format!(
                    " This rule can {}. What happens depends on how many of your cases count under it.",
                    fmt::list(many, "or")
                )),
            };
            msg = msg
                .block(Block::Heading(Text::fixed("Don't post here")))
                .para(sentence)
                .para(Text::fixed(
                    "This channel catches spam accounts. Quack ignores posts from staff and bots.",
                ));
        }
    }
    msg.block(Block::Subtext(Text::fixed(match cases {
        0 => "No honeypot cases yet.".to_owned(),
        1 => "1 honeypot case so far.".to_owned(),
        n => format!("{} honeypot cases so far.", fmt::count(*n)),
    })))
}
