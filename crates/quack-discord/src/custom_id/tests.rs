use proptest::prelude::*;
use quack_core::ids::{
    ActionId, AppealId, CaseId, ChannelId, EvidenceId, JobId, MemberId, MessageId, RuleId, TicketId,
};
use quack_core::rule::FieldKey;
use quack_text::actions::{
    ButtonAction, DraftPage, InputId, ModalAction, SelectAction, SelectValue,
};
use uuid::Uuid;

use super::setup::{AREAS, OPS};
use super::*;

fn uuid() -> impl Strategy<Value = Uuid> {
    any::<u128>().prop_map(Uuid::from_u128)
}

fn snowflake() -> impl Strategy<Value = MemberId> {
    (1..=i64::MAX as u64).prop_map(|v| MemberId::new(v).unwrap())
}

fn draft_page() -> impl Strategy<Value = DraftPage> {
    (uuid(), uuid(), any::<u32>(), any::<u8>()).prop_map(|(d, r, version, page)| DraftPage {
        draft: CaseId::from_uuid(d),
        rule: RuleId::from_uuid(r),
        version,
        page,
    })
}

/// The evidence ids (§11.5 `evs` … `exm`).
fn evidence_id() -> impl Strategy<Value = CustomId> {
    let evidence = || uuid().prop_map(EvidenceId::from_uuid);
    let place = || {
        (1..=i64::MAX as u64, 1..=i64::MAX as u64)
            .prop_map(|(c, m)| (ChannelId::new(c).unwrap(), MessageId::new(m).unwrap()))
    };
    prop_oneof![
        evidence().prop_map(CustomId::EvidenceSurrounding),
        evidence().prop_map(CustomId::BackToEvidence),
        (evidence(), any::<bool>())
            .prop_map(|(evidence, visible)| CustomId::EvidenceVisibility { evidence, visible }),
        evidence().prop_map(CustomId::EditEvidenceNote),
        (evidence(), any::<u32>())
            .prop_map(|(evidence, revision)| CustomId::EvidenceNoteModal { evidence, revision }),
        evidence().prop_map(CustomId::RemoveEvidence),
        evidence().prop_map(CustomId::ConfirmRemoveEvidence),
        place().prop_map(|(channel, message)| CustomId::AddAsEvidence { channel, message }),
        place().prop_map(|(channel, message)| CustomId::AddAsEvidenceByNumber { channel, message }),
        place().prop_map(|(channel, message)| CustomId::AddAsEvidenceModal { channel, message }),
    ]
}

fn custom_id() -> impl Strategy<Value = CustomId> {
    prop_oneof![base_id(), evidence_id()]
}

fn base_id() -> impl Strategy<Value = CustomId> {
    let case = || uuid().prop_map(CaseId::from_uuid);
    let action = || uuid().prop_map(ActionId::from_uuid);
    let appeal = || uuid().prop_map(AppealId::from_uuid);
    let ticket = || uuid().prop_map(TicketId::from_uuid);
    prop_oneof![
        case().prop_map(CustomId::OpenCase),
        Just(CustomId::CasePicker),
        case().prop_map(CustomId::EditContext),
        (case(), any::<u32>())
            .prop_map(|(case, revision)| CustomId::EditContextModal { case, revision }),
        (case(), any::<bool>())
            .prop_map(|(case, new_message)| CustomId::AddEvidence { case, new_message }),
        (case(), any::<bool>())
            .prop_map(|(case, new_message)| CustomId::AddEvidenceModal { case, new_message }),
        (case(), any::<u32>(), any::<bool>()).prop_map(|(case, item, new_message)| {
            CustomId::ViewEvidence {
                case,
                item,
                new_message,
            }
        }),
        case().prop_map(CustomId::VoidCase),
        case().prop_map(CustomId::VoidCaseModal),
        (snowflake(), any::<u32>()).prop_map(|(member, page)| CustomId::History { member, page }),
        (
            any::<u32>(),
            proptest::option::of(snowflake()),
            proptest::option::of(uuid())
        )
            .prop_map(|(page, member, rule)| CustomId::CaseList {
                page,
                member,
                rule: rule.map(RuleId::from_uuid)
            }),
        action().prop_map(CustomId::RetryAction),
        action().prop_map(CustomId::DismissAction),
        action().prop_map(CustomId::UndoAction),
        action().prop_map(CustomId::ConfirmUndo),
        case().prop_map(CustomId::RulePicker),
        (case(), any::<u32>()).prop_map(|(draft, page)| CustomId::RulePickerPage { draft, page }),
        draft_page().prop_map(CustomId::DraftPageModal),
        draft_page().prop_map(CustomId::DraftContinue),
        draft_page().prop_map(CustomId::DraftRetryPage),
        case().prop_map(CustomId::DraftSaveAgain),
        case().prop_map(CustomId::Appeal),
        case().prop_map(CustomId::AppealModal),
        (appeal(), any::<u32>())
            .prop_map(|(appeal, version)| CustomId::AcceptAppeal { appeal, version }),
        (appeal(), any::<u32>())
            .prop_map(|(appeal, version)| CustomId::AcceptAppealModal { appeal, version }),
        (appeal(), any::<u32>())
            .prop_map(|(appeal, version)| CustomId::RejectAppeal { appeal, version }),
        (appeal(), any::<u32>())
            .prop_map(|(appeal, version)| CustomId::RejectAppealModal { appeal, version }),
        appeal().prop_map(CustomId::OpenAppeal),
        any::<u32>().prop_map(CustomId::AppealsPage),
        any::<u32>().prop_map(CustomId::RulesPage),
        (0..AREAS.len(), 0..OPS.len()).prop_map(|(a, o)| CustomId::Setup(AREAS[a], OPS[o])),
        Just(CustomId::SetupInviteModal),
        Just(CustomId::SetupWarningModal),
        Just(CustomId::OpenTicket),
        ticket().prop_map(CustomId::CloseTicket),
        ticket().prop_map(CustomId::FixTicketAccess),
        Just(CustomId::OpsRefresh),
        uuid().prop_map(|j| CustomId::OpsRequeueJob(JobId::from_uuid(j))),
        uuid().prop_map(|j| CustomId::OpsDismissJob(JobId::from_uuid(j))),
        any::<u32>().prop_map(CustomId::OpsJobsPage),
    ]
}

proptest! {
    #[test]
    fn every_id_round_trips_within_the_limit(id in custom_id()) {
        let encoded = id.encode();
        prop_assert!(encoded.len() <= MAX_LEN, "{encoded} is {} characters", encoded.len());
        prop_assert!(encoded.is_ascii());
        prop_assert_eq!(decode(&encoded), ComponentId::Current(id));
    }

    #[test]
    fn hostile_input_never_panics(raw in ".{0,200}") {
        let _ = decode(&raw);
        let _ = decode(&format!("1:{raw}"));
        let _ = decode_input_id(&raw);
    }

    #[test]
    fn mangled_current_ids_are_expired_or_another_valid_id(id in custom_id(), cut in 0_usize..100, junk in "[:a-z0-9_-]{0,3}") {
        let encoded = id.encode();
        let cut = cut.min(encoded.len());
        let mangled = format!("{}{junk}", &encoded[..cut]);
        match decode(&mangled) {
            ComponentId::Current(other) => prop_assert_eq!(other.encode(), mangled),
            ComponentId::Expired | ComponentId::Unrecognized | ComponentId::Legacy(_) => {}
        }
    }
}

#[test]
fn examples_from_the_design() {
    let case = CaseId::from_uuid(Uuid::nil());
    assert_eq!(
        CustomId::OpenCase(case).encode(),
        format!("1:cv:{}", case.to_u22())
    );
    assert_eq!(CustomId::OpenTicket.encode(), "1:to");
    assert_eq!(CustomId::CasePicker.encode(), "1:cvs");
    assert_eq!(
        CustomId::CaseList {
            page: 2,
            member: None,
            rule: None
        }
        .encode(),
        "1:cl:2:0:0"
    );
    assert_eq!(CustomId::SetupInviteModal.encode(), "1:su:ap:invm");
    assert_eq!(CustomId::SetupWarningModal.encode(), "1:su:hp:warnm");
    assert_eq!(
        CustomId::Setup(
            quack_text::actions::SetupArea::Tickets,
            quack_text::actions::SetupOp::TicketQueue
        )
        .encode(),
        "1:su:tk:queue"
    );
    // The longest form.
    let longest = CustomId::DraftPageModal(DraftPage {
        draft: CaseId::from_uuid(Uuid::max()),
        rule: RuleId::from_uuid(Uuid::max()),
        version: u32::MAX,
        page: u8::MAX,
    });
    assert_eq!(longest.encode().len(), 65);
}

#[test]
fn non_canonical_ids_are_expired() {
    let case = CaseId::from_uuid(Uuid::from_u128(7)).to_u22();
    for raw in [
        "1:".to_owned(),
        "1:cv".to_owned(),
        format!("1:cv:{case}:extra"),
        format!("1:ev:{case}:01"),
        format!("1:ev:{case}:+1"),
        format!("1:ev:{case}:4294967296"),
        "1:hi:0:1".to_owned(),
        "1:cl:1:00:0".to_owned(),
        "1:su:ap:nope".to_owned(),
        "1:su:xx:on".to_owned(),
        "1:zz".to_owned(),
        "1:cv:AAAAAAAAAAAAAAAAAAAAA".to_owned(),
    ] {
        assert_eq!(decode(&raw), ComponentId::Expired, "{raw}");
    }
    assert_eq!(decode(&"x".repeat(1_000)), ComponentId::Unrecognized);
    assert_eq!(decode("2:cv:abc"), ComponentId::Unrecognized);
}

#[test]
fn legacy_ids_map_exactly() {
    let ulid = "01ARZ3NDEKTSV4RRFFQ69G5FAV";
    let mapped_id = quack_core::ids::ulid_to_uuid(ulid).unwrap();
    let mapped = |raw: &str| decode(raw);
    assert_eq!(
        mapped(&format!("appeal:submit:v1:{ulid}")),
        ComponentId::Legacy(Legacy::Mapped(CustomId::Appeal(CaseId::from_uuid(
            mapped_id
        ))))
    );
    for verb in ["accept", "reject"] {
        assert_eq!(
            mapped(&format!("appeal:{verb}:v1:{ulid}")),
            ComponentId::Legacy(Legacy::Mapped(CustomId::OpenAppeal(AppealId::from_uuid(
                mapped_id
            ))))
        );
    }
    assert_eq!(
        mapped("ticket:open:v1:"),
        ComponentId::Legacy(Legacy::Mapped(CustomId::OpenTicket))
    );
    assert_eq!(
        mapped(&format!("ticket:close:v1:{ulid}")),
        ComponentId::Legacy(Legacy::Mapped(CustomId::CloseTicket(TicketId::from_uuid(
            mapped_id
        ))))
    );
    assert_eq!(
        mapped(&format!("ticket:repair:v1:{ulid}")),
        ComponentId::Legacy(Legacy::Mapped(CustomId::FixTicketAccess(
            TicketId::from_uuid(mapped_id)
        )))
    );
    for verb in ["view", "evidence", "edit_context", "void"] {
        assert_eq!(
            mapped(&format!("case:{verb}:v1:{ulid}")),
            ComponentId::Legacy(Legacy::Mapped(CustomId::OpenCase(CaseId::from_uuid(
                mapped_id
            ))))
        );
    }
    for verb in ["retry", "dismiss"] {
        assert_eq!(
            mapped(&format!("case:{verb}:v1:{ulid}")),
            ComponentId::Legacy(Legacy::CaseOfAction(ActionId::from_uuid(mapped_id)))
        );
    }
    assert_eq!(
        mapped("case:user_detail:v1:123456789012345678"),
        ComponentId::Legacy(Legacy::Mapped(CustomId::History {
            member: MemberId::new(123_456_789_012_345_678).unwrap(),
            page: 1
        }))
    );
    for raw in [
        "ticket:open:v1",
        "ticket:open:v1:x",
        "ticket:open:v2:",
        "case:view:v1:not-a-ulid",
        "case:user_detail:v1: 12",
        "case:user_detail:v1:0",
        "case:failures:v1:01ARZ3NDEKTSV4RRFFQ69G5FAV",
        "template:edit:v1:01ARZ3NDEKTSV4RRFFQ69G5FAV",
        "",
    ] {
        assert_eq!(mapped(raw), ComponentId::Unrecognized, "{raw}");
    }
}

#[test]
fn text_actions_map_to_their_rows() {
    let case = CaseId::from_uuid(Uuid::from_u128(1));
    assert_eq!(
        CustomId::from(&ButtonAction::OpenCase(case)),
        CustomId::OpenCase(case)
    );
    assert_eq!(CustomId::from(SelectAction::OpenCase), CustomId::CasePicker);
    assert_eq!(
        CustomId::from(SelectAction::PickRule(case)),
        CustomId::RulePicker(case)
    );
    assert_eq!(
        CustomId::from(&ModalAction::VoidCase(case)),
        CustomId::VoidCaseModal(case)
    );
    assert_eq!(
        CustomId::from(&ModalAction::SetupInvite),
        CustomId::SetupInviteModal
    );
}

#[test]
fn input_ids_and_select_values() {
    for id in [
        InputId::Reason,
        InputId::Links,
        InputId::Files,
        InputId::Note,
        InputId::CaseNumber,
        InputId::Statement,
        InputId::Invite,
        InputId::Warning,
        InputId::Field(FieldKey("what_happened".into())),
    ] {
        assert_eq!(decode_input_id(&encode_input_id(&id)), Some(id));
    }
    assert_eq!(decode_input_id("f:Bad Key"), None);
    assert_eq!(decode_input_id("f:"), None);
    assert_eq!(decode_input_id("other"), None);
    let longest = InputId::Field(FieldKey("a".repeat(64)));
    assert!(encode_input_id(&longest).len() <= MAX_LEN);
    assert_eq!(encode_select_value(SelectValue::Yes), "yes");
    assert_eq!(encode_select_value(SelectValue::No), "no");
    let case = CaseId::from_uuid(Uuid::from_u128(9));
    assert_eq!(encode_select_value(SelectValue::Case(case)), case.to_u22());
}

/// Discord review MINOR 19: an overlong `1:` id is answered as an expired
/// button, and an input id never passes 100 characters.
#[test]
fn boundary_ids_follow_the_contract() {
    let long = format!("1:{}", "x".repeat(MAX_LEN * 4));
    assert_eq!(decode(&long), ComponentId::Expired);
    let other = "y".repeat(MAX_LEN * 4 + 1);
    assert_eq!(decode(&other), ComponentId::Unrecognized);
    let key = quack_core::rule::FieldKey("k".repeat(200));
    let id = encode_input_id(&quack_text::actions::InputId::Field(key));
    assert!(id.chars().count() <= MAX_LEN, "{}", id.len());
}
