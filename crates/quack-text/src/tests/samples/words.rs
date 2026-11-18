//! §12.1 commands, §12.2 shared words, §12.12 help and rules.

use quack_core::action::ActionKind;
use quack_core::rule::LevelAction;

use super::{Out, Sample, covering, label, msg, sample, text};
use crate::budget::{BUTTON_LABEL, CHOICE_NAME, COMMAND_DESCRIPTION, SELECT_PLACEHOLDER};
use crate::command;
use crate::help::{self, Topic};
use crate::model::Msg;
use crate::rules::{self, RuleRow};
use crate::tests::fixtures::*;
use crate::word::{self, CaseOption, Outcome};

pub fn samples() -> Vec<Sample> {
    let mut v = Vec::new();
    let ids = [
        "command.case",
        "command.case.add",
        "command.case.add.member",
        "command.case.add.rule",
        "command.case.add.private",
        "command.case.view",
        "command.case.number",
        "command.case.list",
        "command.case.list.member",
        "command.case.list.rule",
        "command.case.void",
        "command.case.void.reason",
        "command.case.history",
        "command.case.history.member",
        "command.appeals",
        "command.rules",
        "command.setup",
        "command.help",
        "command.help.topic",
    ];
    for (id, c) in ids.into_iter().zip(command::ALL) {
        v.push(label(
            id,
            format!("{} — {}", c.name, c.description),
            COMMAND_DESCRIPTION + 40,
        ));
        v.push(sample(
            id,
            "description",
            Out::Str(c.description.to_owned(), COMMAND_DESCRIPTION),
        ));
    }
    for (id, c) in [
        "command.help.topic.cases",
        "command.help.topic.rules",
        "command.help.topic.appeals",
        "command.help.topic.setup",
    ]
    .into_iter()
    .zip(command::HELP_TOPICS)
    {
        v.push(label(id, c.name, CHOICE_NAME));
    }
    v.push(label("command.add_case", command::ADD_CASE, 32));

    for (id, kind) in [
        ("word.action.timeout", ActionKind::Timeout),
        ("word.action.kick", ActionKind::Kick),
        ("word.action.ban", ActionKind::Ban),
        ("word.action.remove_timeout", ActionKind::RemoveTimeout),
        ("word.action.unban", ActionKind::Unban),
    ] {
        v.push(label(id, word::action(kind), BUTTON_LABEL));
    }
    for (id, o) in [
        ("word.outcome.in_progress", Outcome::InProgress),
        ("word.outcome.done", Outcome::Done),
        ("word.outcome.failed", Outcome::Failed),
        ("word.outcome.couldnt_confirm", Outcome::CouldntConfirm),
        ("word.outcome.not_needed", Outcome::NotNeeded),
        ("word.outcome.cancelled", Outcome::Cancelled),
        ("word.outcome.left_in_place", Outcome::LeftInPlace),
    ] {
        v.push(label(id, word::outcome(o), BUTTON_LABEL));
    }
    v.push(label("word.yes", word::yes_no(true), 100));
    v.push(label("word.no", word::yes_no(false), 100));
    v.push(label("word.voided", word::VOIDED, 100));
    v.push(label("word.imported", word::IMPORTED, 100));
    v.push(label("word.page", word::page(2, 5), 100));
    v.push(covering(
        "button.pager",
        &["button.previous", "button.next"],
        "middle page",
        Out::Msg(Msg::default().buttons(word::pager(
            2,
            3,
            crate::actions::ButtonAction::RulesPage,
        ))),
    ));
    v.push(covering(
        "button.links",
        &[
            "button.open_case",
            "button.open_in_dashboard",
            "button.failed_actions",
        ],
        "",
        Out::Msg(Msg::default().buttons(vec![
            word::open_case_button(case_id()),
            word::dashboard_button(URL),
            word::failed_actions_button(URL),
        ])),
    ));
    let options = vec![CaseOption {
        case: case_id(),
        number: 12,
        rule: "Spam".to_owned(),
    }];
    v.push(msg(
        "select.open_case",
        "",
        Msg::default().select(word::open_case_select(&options).unwrap()),
    ));
    v.push(sample(
        "select.open_case",
        "placeholder",
        Out::Str(
            word::open_case_select(&options).unwrap().placeholder,
            SELECT_PLACEHOLDER,
        ),
    ));
    v.push(text("render.trimmed", "", word::render_trimmed()));
    let levels = [
        (1, None),
        (
            3,
            Some(LevelAction::Timeout {
                duration_seconds: 5_400,
            }),
        ),
        (4, Some(LevelAction::Kick)),
        (
            5,
            Some(LevelAction::Ban {
                delete_message_seconds: 0,
            }),
        ),
        (
            6,
            Some(LevelAction::Ban {
                delete_message_seconds: 86_400,
            }),
        ),
    ];
    for (id, (from, action)) in [
        "level.outcome.warning",
        "level.outcome.timeout",
        "level.outcome.kick",
        "level.outcome.ban",
        "level.outcome.ban_delete",
    ]
    .into_iter()
    .zip(levels)
    {
        v.push(label(id, word::level_outcome(action), 200));
        v.push(sample(
            "level.summary.item",
            id,
            Out::Str(word::level_summary_item(from, action), 200),
        ));
    }

    v.push(msg("help.overview", "", help::help(None, URL)));
    v.push(covering(
        "help.cases",
        &["help.dashboard_link"],
        "",
        Out::Msg(help::help(Some(Topic::Cases), URL)),
    ));
    v.push(msg("help.rules", "", help::help(Some(Topic::Rules), URL)));
    v.push(msg(
        "help.appeals",
        "",
        help::help(Some(Topic::Appeals), URL),
    ));
    v.push(msg("help.setup", "", help::help(Some(Topic::Setup), URL)));

    let row = RuleRow {
        name: "Spam".to_owned(),
        levels: levels.to_vec(),
        url: URL.to_owned(),
    };
    let rows: Vec<RuleRow> = (0..10)
        .map(|i| RuleRow {
            name: format!("{} {i}", long(90)),
            ..row.clone()
        })
        .collect();
    v.push(covering(
        "rules.list",
        &["rules.heading", "rules.row", "rules.footer"],
        "one rule, one archived",
        Out::Msg(rules::list(std::slice::from_ref(&row), 1, 1, 1)),
    ));
    v.push(sample(
        "rules.list",
        "page 2 of 3, three archived",
        Out::Msg(rules::list(&[row], 3, 2, 3)),
    ));
    v.push(sample(
        "rules.list",
        "maximal page",
        Out::Msg(rules::list(&rows, 0, 1, 2)),
    ));
    v.push(covering(
        "rules.empty",
        &[],
        "",
        Out::Msg(rules::list(&[], 0, 1, 1)),
    ));
    for code in quack_core::rule::RuleProblemCode::ALL {
        v.push(text("rule.problem", code.as_str(), rules::problem(*code)));
    }
    v
}
