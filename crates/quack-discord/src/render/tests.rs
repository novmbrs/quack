use proptest::prelude::*;
use quack_core::ids::{AccountId, CaseId, ChannelId, RuleId};
use quack_core::rule::{FieldKey, FieldKind};
use quack_text::actions::{ButtonAction, DraftPage, SelectAction};
use quack_text::case::{FormField, PickerRule};
use quack_text::help::Topic;
use quack_text::model::{
    Block, Button, ButtonStyle, ChannelRef, Command, Person, Row, Select, SelectOptions, Text,
};
use quack_text::{CommandIds, Msg, Tone};
use twilight_model::channel::message::Component;
use uuid::Uuid;

use super::*;

fn ids() -> CommandIds {
    CommandIds(vec![
        (Command::CaseView, 1_000_000_000_000_000_001),
        (Command::Setup, 1_000_000_000_000_000_002),
    ])
}

fn json(rendered: &RenderedMessage) -> serde_json::Value {
    serde_json::to_value(&rendered.components).unwrap()
}

fn case(n: u128) -> CaseId {
    CaseId::from_uuid(Uuid::from_u128(n))
}

/// Every text display's content, and every custom id, of a rendering.
fn walk(components: &[Component], text: &mut usize, custom_ids: &mut Vec<String>) {
    for c in components {
        match c {
            Component::Container(c) => walk(&c.components, text, custom_ids),
            Component::ActionRow(r) => walk(&r.components, text, custom_ids),
            Component::TextDisplay(t) => *text += t.content.chars().count(),
            Component::Button(b) => custom_ids.extend(b.custom_id.clone()),
            Component::SelectMenu(s) => custom_ids.push(s.custom_id.clone()),
            _ => {}
        }
    }
}

#[test]
fn every_block_kind_and_tone() {
    let msg = Msg::new(Tone::Danger)
        .block(Block::Heading(Text::fixed("Case #12 · Spam")))
        .block(Block::Text(
            Text::fixed("Quack couldn't ban ")
                .person(&Person::new(AccountId::new(9).unwrap(), "x")),
        ))
        .block(Block::Field {
            label: "Reason".into(),
            value: Text::new().user("*bold* attempt <@1>"),
        })
        .block(Block::Quote(Text::new().user("line one\nline two")))
        .block(Block::Subtext(
            Text::fixed("Run ").command(Command::CaseView),
        ))
        .block(Block::Divider)
        .block(Block::File("transcript.txt".into()))
        .block(Block::Text(Text::fixed("after the file")))
        .buttons(vec![
            Button::new(
                "Void case",
                ButtonStyle::Danger,
                ButtonAction::VoidCase(case(1)),
            ),
            Button::link(
                "Open in dashboard",
                "https://quack.example/guilds/1/cases/12",
            ),
        ])
        .select(Select {
            action: SelectAction::Setup(
                quack_text::actions::SetupArea::Log,
                quack_text::actions::SetupOp::Channel,
            ),
            placeholder: "Choose a channel".into(),
            options: SelectOptions::TextChannels,
        });
    insta::assert_json_snapshot!(json(&render(&msg, &ids())));
}

#[test]
fn accents_follow_the_tone() {
    assert_eq!(accent(Tone::Neutral), None);
    assert_eq!(accent(Tone::Success), Some(0x3B_A5_5C));
    assert_eq!(accent(Tone::Warning), Some(0xFA_A6_1A));
    assert_eq!(accent(Tone::Danger), Some(0xED_42_45));
}

#[test]
fn help_overview_and_topic() {
    insta::assert_json_snapshot!(
        "help_overview",
        json(&render(
            &quack_text::help::help(None, "https://quack.example"),
            &ids()
        ))
    );
    insta::assert_json_snapshot!(
        "help_setup",
        json(&render(
            &quack_text::help::help(Some(Topic::Setup), "https://quack.example"),
            &ids()
        ))
    );
}

#[test]
fn ticket_panel_and_rule_picker() {
    insta::assert_json_snapshot!(
        "ticket_panel",
        json(&render(
            &quack_text::ticket::panel(true),
            &CommandIds::EMPTY
        ))
    );
    let rules = [
        PickerRule {
            id: RuleId::from_uuid(Uuid::from_u128(5)),
            name: "Spam".into(),
            description: "Unwanted ads".into(),
        },
        PickerRule {
            id: RuleId::from_uuid(Uuid::from_u128(6)),
            name: "Harassment".into(),
            description: String::new(),
        },
    ];
    let member = Person::new(AccountId::new(9).unwrap(), "member");
    insta::assert_json_snapshot!(
        "rule_picker",
        json(&render(
            &quack_text::case::picker(&member, case(2), &rules, 1, 2),
            &CommandIds::EMPTY
        ))
    );
}

#[test]
fn modals_use_labels() {
    let void = render_modal(&quack_text::case::void_modal(case(3), Some(12)));
    insta::assert_json_snapshot!(
        "void_modal",
        (&void.custom_id, &void.title, &void.components)
    );
    let page = DraftPage {
        draft: case(4),
        rule: RuleId::from_uuid(Uuid::from_u128(5)),
        version: 3,
        page: 1,
    };
    let fields = [
        FormField {
            key: FieldKey("what".into()),
            label: "What happened?".into(),
            kind: FieldKind::LongText,
            required: true,
            value: None,
        },
        FormField {
            key: FieldKey("repeat".into()),
            label: "Did it happen before?".into(),
            kind: FieldKind::Boolean,
            required: false,
            value: Some(quack_core::case::ContextValue::Boolean(true)),
        },
    ];
    let form = render_modal(&quack_text::case::form_modal("Spam", page, 2, &fields));
    let wire = form.wire_components();
    // Discord review MAJOR 6: Discord forbids `disabled` on a modal select,
    // whatever its value; asserted apart from the snapshot so accepting a
    // snapshot can't bless it.
    assert!(!wire.to_string().contains("\"disabled\""), "{wire}");
    insta::assert_json_snapshot!("case_form_modal", (&form.custom_id, &form.title, &wire));
}

#[test]
fn an_empty_message_still_has_a_text_display() {
    let rendered = render(&Msg::new(Tone::Neutral), &CommandIds::EMPTY);
    assert_eq!(component_count(&rendered.components), 2);
}

#[test]
fn rows_past_the_component_budget_are_dropped() {
    let mut msg = Msg::new(Tone::Neutral);
    for i in 0..10 {
        msg = msg
            .block(Block::Text(Text::fixed(format!("t{i}"))))
            .block(Block::Divider);
    }
    let buttons = (0..30)
        .map(|i| {
            Button::new(
                format!("b{i}"),
                ButtonStyle::Secondary,
                ButtonAction::RulesPage(i),
            )
        })
        .collect();
    let rendered = render(&msg.buttons(buttons), &CommandIds::EMPTY);
    assert!(component_count(&rendered.components) <= quack_text::budget::COMPONENT_BUDGET);
}

fn block() -> impl Strategy<Value = Block> {
    let text = || ".{0,600}".prop_map(|s| Text::new().user(s));
    prop_oneof![
        text().prop_map(Block::Heading),
        text().prop_map(Block::Text),
        text().prop_map(Block::Quote),
        text().prop_map(Block::Subtext),
        text().prop_map(|value| Block::Field {
            label: "Label".into(),
            value
        }),
        Just(Block::Divider),
        "[a-z]{1,20}".prop_map(|n| Block::File(format!("{n}.txt"))),
        any::<u64>().prop_map(
            |id| Block::Text(Text::fixed("in ").channel(&ChannelRef::new(
                ChannelId::new(id % 1_000_000 + 1).unwrap(),
                "c"
            )))
        ),
    ]
}

fn row() -> impl Strategy<Value = Row> {
    prop_oneof![
        (1_u32..=5).prop_map(|n| Row::Buttons(
            (0..n)
                .map(|i| Button::new(
                    "x".repeat(90),
                    ButtonStyle::Primary,
                    ButtonAction::AppealsPage(u32::MAX - i)
                ))
                .collect()
        )),
        Just(Row::Select(Select {
            action: SelectAction::OpenCase,
            placeholder: "Open a case".into(),
            options: SelectOptions::TextChannels,
        })),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn any_message_fits_discords_limits(
        tone in prop_oneof![Just(Tone::Neutral), Just(Tone::Success), Just(Tone::Warning), Just(Tone::Danger)],
        blocks in proptest::collection::vec(block(), 0..30),
        rows in proptest::collection::vec(row(), 0..3),
    ) {
        let msg = Msg { tone, blocks, rows, trimmed: None, ping: None };
        let rendered = render(&msg, &ids());
        prop_assert!(component_count(&rendered.components) <= quack_text::budget::COMPONENT_BUDGET);
        let (mut text, mut custom_ids) = (0, Vec::new());
        walk(&rendered.components, &mut text, &mut custom_ids);
        prop_assert!(text <= quack_text::budget::TEXT_BUDGET, "{text} characters");
        for id in custom_ids {
            prop_assert!(id.len() <= crate::custom_id::MAX_LEN);
        }
    }
}

/// Evidence galleries (§11.11) are Media Galleries whose items are
/// spoilered; a URL that isn't `https://` is left out.
#[test]
fn a_gallery_is_a_spoilered_media_gallery() {
    use quack_text::model::GalleryItem;
    let item = |url: &str| GalleryItem {
        url: url.to_owned(),
        description: Some("proof.png".to_owned()),
        spoiler: true,
    };
    let msg = Msg::new(Tone::Neutral)
        .block(Block::Text(Text::fixed("before")))
        .block(Block::Gallery(vec![
            item("https://cdn.discordapp.com/attachments/1/2/SPOILER_proof.png"),
            item("javascript:alert(1)"),
        ]));
    let rendered = render(&msg, &ids());
    let value = json(&rendered);
    let gallery = &value[0]["components"][1];
    assert_eq!(gallery["type"], 12, "{value}");
    assert_eq!(gallery["items"].as_array().map(Vec::len), Some(1));
    assert_eq!(gallery["items"][0]["spoiler"], true);
    assert_eq!(gallery["items"][0]["description"], "proof.png");
}

/// The add-evidence modal's file field is a File Upload in a Label.
#[test]
fn the_files_input_is_a_file_upload() {
    let modal = render_modal(&quack_text::evidence::modal(case(4), 12, false));
    let wire = modal.wire_components();
    let upload = &wire[1]["component"];
    assert_eq!(upload["type"], 19, "{wire}");
    assert_eq!(upload["custom_id"], "files");
    assert_eq!(upload["max_values"], 10);
    assert_eq!(upload["min_values"], 0);
    assert_eq!(upload["required"], false);
    assert_eq!(wire[2]["component"]["custom_id"], "note");
}
