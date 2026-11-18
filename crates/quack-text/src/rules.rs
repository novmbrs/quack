//! `/rules` (§12.12): the server's active rules with a dashboard link each.

use quack_core::rule::LevelAction;

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
