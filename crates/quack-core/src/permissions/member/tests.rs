use super::*;

const OWNER: u64 = 1;
const ACTOR: u64 = 2;
const MEMBER: u64 = 3;

#[allow(
    clippy::unnecessary_wraps,
    reason = "matches the Option fields it fills"
)]
fn present(position: u32) -> Option<PresentMember> {
    Some(PresentMember {
        highest_role_position: position,
        permissions: Permissions::NONE,
    })
}

#[allow(
    clippy::unnecessary_wraps,
    reason = "matches the Option fields it fills"
)]
fn quack_with(perms: &[Permission]) -> Option<QuackFacts> {
    Some(PresentMember {
        highest_role_position: 10,
        permissions: perms.iter().copied().collect(),
    })
}

/// Facts where every check passes: Quack at 10 with Administrator, the
/// actor at 5, the member at 2.
fn ok() -> ApplyFacts {
    ApplyFacts {
        owner_id: AccountId::new(OWNER).unwrap(),
        quack: quack_with(&[Permission::Administrator]),
        actor: ActorPresence::Staff {
            id: AccountId::new(ACTOR).unwrap(),
            member: present(5),
        },
        member: MemberSubject {
            id: MemberId::new(MEMBER).unwrap(),
            bot: false,
            present: present(2),
        },
    }
}

fn with(edit: impl FnOnce(&mut ApplyFacts)) -> ApplyFacts {
    let mut f = ok();
    edit(&mut f);
    f
}

fn member_id(n: u64) -> MemberId {
    MemberId::new(n).unwrap()
}

#[test]
fn apply_rule_checks_in_order() {
    use MemberRefusal as R;
    let cases: Vec<(&str, ApplyFacts, Result<(), MemberRefusal>)> = vec![
        ("all pass", ok(), Ok(())),
        (
            "1 quack absent",
            with(|f| f.quack = None),
            Err(R::QuackNotInServer),
        ),
        (
            "2 actor absent",
            with(|f| {
                f.actor = ActorPresence::Staff {
                    id: AccountId::new(ACTOR).unwrap(),
                    member: None,
                }
            }),
            Err(R::ActorNotInServer),
        ),
        (
            "3 member absent",
            with(|f| f.member.present = None),
            Err(R::MemberNotInServer),
        ),
        (
            "4 self",
            with(|f| f.member.id = member_id(ACTOR)),
            Err(R::MemberIsSelf),
        ),
        ("5 bot", with(|f| f.member.bot = true), Err(R::MemberIsBot)),
        (
            "6 owner",
            with(|f| f.member.id = member_id(OWNER)),
            Err(R::MemberIsOwner),
        ),
        (
            "7 equal role",
            with(|f| f.member.present = present(5)),
            Err(R::MemberRoleTooHigh),
        ),
        (
            "7 higher role",
            with(|f| f.member.present = present(7)),
            Err(R::MemberRoleTooHigh),
        ),
        (
            "8 quack too low",
            with(|f| {
                f.actor = ActorPresence::Staff {
                    id: AccountId::new(ACTOR).unwrap(),
                    member: present(20),
                };
                f.member.present = present(10);
            }),
            Err(R::QuackRoleTooLow),
        ),
        (
            "1 wins over 3",
            with(|f| {
                f.quack = None;
                f.member.present = None;
            }),
            Err(R::QuackNotInServer),
        ),
        (
            "5 wins over 6",
            with(|f| {
                f.member.id = member_id(OWNER);
                f.member.bot = true;
            }),
            Err(R::MemberIsBot),
        ),
    ];
    for (name, facts, expected) in cases {
        assert_eq!(
            check_member(&facts, CheckPurpose::ApplyRule),
            expected,
            "{name}"
        );
    }
}

#[test]
fn owner_actor_skips_role_hierarchy_but_not_quacks() {
    let facts = with(|f| {
        f.actor = ActorPresence::Staff {
            id: AccountId::new(OWNER).unwrap(),
            member: present(0),
        };
        f.member.present = present(9);
    });
    assert_eq!(check_member(&facts, CheckPurpose::ApplyRule), Ok(()));
    let facts = with(|f| {
        f.actor = ActorPresence::Staff {
            id: AccountId::new(OWNER).unwrap(),
            member: present(0),
        };
        f.member.present = present(10);
    });
    assert_eq!(
        check_member(&facts, CheckPurpose::ApplyRule),
        Err(MemberRefusal::QuackRoleTooLow)
    );
}

#[test]
fn honeypot_runs_1_3_5_6_8() {
    let p = CheckPurpose::ApplyHoneypotRule;
    let quack_actor = |f: &mut ApplyFacts| f.actor = ActorPresence::Quack;
    assert_eq!(check_member(&with(quack_actor), p), Ok(()));
    assert_eq!(
        check_member(
            &with(|f| {
                quack_actor(f);
                f.quack = None;
            }),
            p
        ),
        Err(MemberRefusal::QuackNotInServer)
    );
    assert_eq!(
        check_member(
            &with(|f| {
                quack_actor(f);
                f.member.present = None;
            }),
            p
        ),
        Err(MemberRefusal::MemberNotInServer)
    );
    assert_eq!(
        check_member(
            &with(|f| {
                quack_actor(f);
                f.member.bot = true;
            }),
            p
        ),
        Err(MemberRefusal::MemberIsBot)
    );
    assert_eq!(
        check_member(
            &with(|f| {
                quack_actor(f);
                f.member.id = member_id(OWNER);
            }),
            p
        ),
        Err(MemberRefusal::MemberIsOwner)
    );
    assert_eq!(
        check_member(
            &with(|f| {
                quack_actor(f);
                f.member.present = present(10);
            }),
            p
        ),
        Err(MemberRefusal::QuackRoleTooLow)
    );
    // Checks 2, 4 and 7 never run for the honeypot.
    let staff_facts = with(|f| {
        f.actor = ActorPresence::Staff {
            id: AccountId::new(MEMBER).unwrap(),
            member: None,
        };
        f.member.present = present(9);
    });
    assert_eq!(check_member(&staff_facts, p), Ok(()));
}

#[test]
fn retry_runs_2_4_6_7_and_3_for_timeouts() {
    let kick = CheckPurpose::RetryAction(ActionKind::Kick);
    let timeout = CheckPurpose::RetryAction(ActionKind::Timeout);
    let absent = with(|f| f.member.present = None);
    assert_eq!(check_member(&absent, kick), Ok(()));
    assert_eq!(
        check_member(&absent, timeout),
        Err(MemberRefusal::MemberNotInServer)
    );
    // Quack's own checks (1, 8) and the bot check (5) are skipped.
    let skipped = with(|f| {
        f.quack = None;
        f.member.bot = true;
    });
    assert_eq!(check_member(&skipped, kick), Ok(()));
    assert_eq!(
        check_member(&with(|f| f.member.id = member_id(ACTOR)), kick),
        Err(MemberRefusal::MemberIsSelf)
    );
    assert_eq!(
        check_member(&with(|f| f.member.id = member_id(OWNER)), kick),
        Err(MemberRefusal::MemberIsOwner)
    );
    assert_eq!(
        check_member(&with(|f| f.member.present = present(6)), kick),
        Err(MemberRefusal::MemberRoleTooHigh)
    );
    let gone_actor = with(|f| {
        f.actor = ActorPresence::Staff {
            id: AccountId::new(ACTOR).unwrap(),
            member: None,
        }
    });
    assert_eq!(
        check_member(&gone_actor, kick),
        Err(MemberRefusal::ActorNotInServer)
    );
}

#[test]
fn absent_member_skips_role_positions() {
    let facts = with(|f| f.member.present = None);
    assert_eq!(
        check_member(&facts, CheckPurpose::RetryAction(ActionKind::Ban)),
        Ok(())
    );
}

#[test]
fn action_permission_checks() {
    use ActionKind as K;
    let quack_only = |perms: &[Permission]| with(|f| f.quack = quack_with(perms));
    let cases = [
        (K::Timeout, &[Permission::ModerateMembers][..], Ok(())),
        (
            K::Timeout,
            &[Permission::BanMembers][..],
            Err(MemberRefusal::QuackMissingPermission(
                DiscordPermission::ModerateMembers,
            )),
        ),
        (K::Kick, &[Permission::KickMembers][..], Ok(())),
        (
            K::Kick,
            &[][..],
            Err(MemberRefusal::QuackMissingPermission(
                DiscordPermission::KickMembers,
            )),
        ),
        (K::Ban, &[Permission::BanMembers][..], Ok(())),
        (
            K::Ban,
            &[Permission::KickMembers][..],
            Err(MemberRefusal::QuackMissingPermission(
                DiscordPermission::BanMembers,
            )),
        ),
        (K::Ban, &[Permission::Administrator][..], Ok(())),
    ];
    for (kind, perms, expected) in cases {
        assert_eq!(
            check_action_permission(&quack_only(perms), kind),
            expected,
            "{kind:?} {perms:?}"
        );
    }
    assert_eq!(
        check_action_permission(&with(|f| f.quack = None), K::Kick),
        Err(MemberRefusal::QuackNotInServer)
    );

    let admin_member = with(|f| {
        f.member.present = Some(PresentMember {
            highest_role_position: 1,
            permissions: Permission::Administrator.into(),
        });
    });
    assert_eq!(
        check_action_permission(&admin_member, K::Timeout),
        Err(MemberRefusal::MemberIsAdministrator)
    );
    assert_eq!(check_action_permission(&admin_member, K::Ban), Ok(()));
}

#[test]
fn refusal_codes_and_quack_access() {
    assert!(MemberRefusal::QuackNotInServer.is_quack_access());
    assert!(MemberRefusal::QuackRoleTooLow.is_quack_access());
    assert!(MemberRefusal::QuackMissingPermission(DiscordPermission::BanMembers).is_quack_access());
    for r in [
        MemberRefusal::ActorNotInServer,
        MemberRefusal::MemberNotInServer,
        MemberRefusal::MemberIsSelf,
        MemberRefusal::MemberIsBot,
        MemberRefusal::MemberIsOwner,
        MemberRefusal::MemberRoleTooHigh,
        MemberRefusal::MemberIsAdministrator,
    ] {
        assert!(!r.is_quack_access(), "{r:?}");
        assert_eq!(r.permission(), None);
    }
    assert_eq!(
        MemberRefusal::QuackMissingPermission(DiscordPermission::KickMembers)
            .code()
            .as_str(),
        "quack_missing_permission"
    );
    assert_eq!(
        MemberRefusal::MemberRoleTooHigh.code().as_str(),
        "member_role_too_high"
    );
}

#[test]
fn precheck_order_and_reasons() {
    use ActionFailure as F;
    use ActionKind as K;
    let reason = |facts: &ApplyFacts, kind| action_precheck(facts, kind).map_err(|f| f.reason);
    let quack_actor = |f: &mut ApplyFacts| f.actor = ActorPresence::Quack;
    assert_eq!(reason(&with(quack_actor), K::Timeout), Ok(()));
    assert_eq!(
        reason(&with(|f| f.quack = None), K::Ban),
        Err(F::QuackNotInServer)
    );
    let no_perm = action_precheck(&with(|f| f.quack = quack_with(&[])), K::Kick).unwrap_err();
    assert_eq!(
        (no_perm.reason, no_perm.permission),
        (
            F::QuackMissingPermission,
            Some(DiscordPermission::KickMembers)
        )
    );
    assert_eq!(
        reason(&with(|f| f.member.id = member_id(OWNER)), K::Ban),
        Err(F::MemberIsOwner)
    );
    // A timeout's absent or administrator member is found by the
    // attempt's own read (§5.1), not by the pre-check.
    assert_eq!(
        reason(&with(|f| f.member.present = None), K::Timeout),
        Ok(())
    );
    assert_eq!(reason(&with(|f| f.member.present = None), K::Ban), Ok(()));
    assert_eq!(
        reason(&with(|f| f.member.present = None), K::RemoveTimeout),
        Ok(())
    );
    let admin = with(|f| {
        f.member.present = Some(PresentMember {
            highest_role_position: 1,
            permissions: Permission::Administrator.into(),
        });
    });
    assert_eq!(reason(&admin, K::Timeout), Ok(()));
    let high = with(|f| f.member.present = present(10));
    assert_eq!(reason(&high, K::Kick), Err(F::QuackRoleTooLow));
    assert_eq!(reason(&high, K::RemoveTimeout), Err(F::QuackRoleTooLow));
    assert_eq!(reason(&high, K::Unban), Ok(()));
}
