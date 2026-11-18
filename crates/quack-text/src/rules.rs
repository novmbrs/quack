//! `/rules` (§12.12): the server's active rules with a dashboard link each,
//! and the messages for problems with a rule definition (§3.3).

use quack_core::rule::{LevelAction, RuleProblemCode};

use crate::actions::ButtonAction;
use crate::fmt;
use crate::model::{Block, Msg, Text, Tone};
use crate::word;

/// One active rule in `/rules`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuleRow {
    /// Its name.
    pub name: String,
    /// Each level's starting case count and action.
    pub levels: Vec<(u32, Option<LevelAction>)>,
    /// The rule on the dashboard.
    pub url: String,
}

/// `rules.*`: one page of active rules (10 per page).
pub fn list(rows: &[RuleRow], archived: u64, page: u32, pages: u32) -> Msg {
    let mut msg = Msg::new(Tone::Neutral).block(Block::Heading(Text::fixed("Rules")));
    if rows.is_empty() {
        msg = msg.para(Text::fixed(
            "This server has no active rules. Create one on the dashboard.",
        ));
    }
    for r in rows {
        msg = msg.para(
            Text::new()
                .bold_user(fmt::one_line(&r.name))
                .s(" · ")
                .s(word::levels_summary(&r.levels))
                .s(" · ")
                .link("Open", &r.url),
        );
    }
    let mut footer = Text::fixed("Rules are edited on the dashboard.");
    footer = match archived {
        0 => footer,
        1 => footer.s(" 1 archived rule is there too."),
        n => footer.s(format!(" {} archived rules are there too.", fmt::count(n))),
    };
    if pages > 1 {
        footer = footer.s(" ").s(word::page(page, pages)).s(".");
    }
    msg.block(Block::Subtext(footer))
        .buttons(word::pager(page, pages, ButtonAction::RulesPage))
}

/// `rule.problem.*`: one problem with a rule definition (§3.3), the
/// `message` of a `validation_failed` field on the dashboard's rule editor.
pub fn problem(code: RuleProblemCode) -> Text {
    Text::fixed(match code {
        RuleProblemCode::NameLength => "Give the rule a name of 1 to 100 characters.",
        RuleProblemCode::DescriptionLength => "Keep the description to 500 characters or fewer.",
        RuleProblemCode::ReasonLength => "Give the rule a reason of 1 to 1,000 characters.",
        RuleProblemCode::CountingWindow => {
            "Count cases from the last 1 to 36,500 days, or count every case."
        }
        RuleProblemCode::TooManyFields => "A rule can have at most 10 context fields.",
        RuleProblemCode::FieldKey => {
            "Use 2 to 64 lowercase letters, digits, dashes, or underscores for the key, starting with a letter or digit."
        }
        RuleProblemCode::FieldKeyDuplicate => "Another field of this rule already uses this key.",
        RuleProblemCode::FieldLabelLength => "Give the field a label of 1 to 100 characters.",
        RuleProblemCode::LevelCount => "A rule needs 1 to 10 levels.",
        RuleProblemCode::FirstLevelNotOne => "The first level must start at case 1.",
        RuleProblemCode::LevelCaseCount => {
            "Start each later level at a case count from 2 to 1,000,000."
        }
        RuleProblemCode::LevelOrder => {
            "Start each level at a higher case count than the level before it."
        }
        RuleProblemCode::TimeoutDuration => "A timeout lasts from 1 second to 28 days.",
        RuleProblemCode::BanDeleteWindow => "A ban can delete at most the last 7 days of messages.",
    })
}
