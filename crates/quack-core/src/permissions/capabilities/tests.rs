use super::*;

fn role(n: u64) -> RoleId {
    RoleId::new(n).unwrap()
}

fn actor(perms: &[Permission], roles: &[u64]) -> ActorFacts {
    ActorFacts {
        is_owner: false,
        permissions: perms.iter().copied().collect(),
        role_ids: roles.iter().map(|&r| role(r)).collect(),
        mfa_enabled: true,
    }
}

fn staff(moderators: &[u64], managers: &[u64]) -> StaffRoles {
    StaffRoles {
        moderator_role_ids: moderators.iter().map(|&r| role(r)).collect(),
        rules_manager_role_ids: managers.iter().map(|&r| role(r)).collect(),
    }
}

/// (who, configured roles) → [`moderate`, `manage_rules`, `manage_settings`, `manage_staff_roles`]
#[test]
fn capability_table() {
    let owner = ActorFacts {
        is_owner: true,
        ..actor(&[], &[])
    };
    let cases: Vec<(&str, ActorFacts, StaffRoles, [bool; 4])> = vec![
        ("owner", owner, staff(&[], &[]), [true, true, true, true]),
        (
            "administrator",
            actor(&[Permission::Administrator], &[]),
            staff(&[10], &[]),
            [true, true, true, true],
        ),
        (
            "manage server",
            actor(&[Permission::ManageGuild], &[]),
            staff(&[], &[]),
            [false, true, true, false],
        ),
        (
            "moderator role",
            actor(&[], &[10]),
            staff(&[10], &[]),
            [true, false, false, false],
        ),
        (
            "rules manager role",
            actor(&[], &[20]),
            staff(&[], &[20]),
            [false, true, false, false],
        ),
        (
            "moderate members, no roles configured",
            actor(&[Permission::ModerateMembers], &[]),
            staff(&[], &[]),
            [true, false, false, false],
        ),
        (
            "moderate members, roles configured",
            actor(&[Permission::ModerateMembers], &[]),
            staff(&[10], &[]),
            [false, false, false, false],
        ),
        (
            "nobody",
            actor(&[Permission::KickMembers], &[30]),
            staff(&[10], &[20]),
            [false, false, false, false],
        ),
        (
            "combined",
            actor(&[Permission::ManageGuild], &[10]),
            staff(&[10], &[]),
            [true, true, true, false],
        ),
    ];
    for (who, facts, roles, [m, r, s, st]) in cases {
        let caps = derive_capabilities(&facts, &roles, false);
        assert_eq!(
            [
                caps.moderate,
                caps.manage_rules,
                caps.manage_settings,
                caps.manage_staff_roles
            ],
            [m, r, s, st],
            "{who}"
        );
    }
}

#[test]
fn deleted_moderator_roles_fall_back_to_moderate_members() {
    let facts = actor(&[Permission::ModerateMembers], &[]);
    let configured = staff(&[10], &[20]);
    assert!(!derive_capabilities(&facts, &configured, false).moderate);
    let existing = configured.existing(&[role(99)]);
    assert_eq!(existing, staff(&[], &[]));
    assert!(derive_capabilities(&facts, &existing, false).moderate);
}

#[test]
fn mfa_rule_applies_to_everyone_including_the_owner() {
    let owner = ActorFacts {
        is_owner: true,
        mfa_enabled: false,
        ..actor(&[], &[])
    };
    let caps = derive_capabilities(&owner, &StaffRoles::default(), true);
    assert!(caps.holds(Capability::Moderate));
    assert_eq!(
        caps.check(Capability::Moderate),
        Err(AccessRefusal::MfaRequired)
    );
    let caps = derive_capabilities(&owner, &StaffRoles::default(), false);
    assert_eq!(caps.check(Capability::Moderate), Ok(()));

    let nobody = ActorFacts {
        mfa_enabled: false,
        ..actor(&[], &[])
    };
    let caps = derive_capabilities(&nobody, &StaffRoles::default(), true);
    assert_eq!(
        caps.check(Capability::ManageRules),
        Err(AccessRefusal::MissingCapability(Capability::ManageRules))
    );
    assert_eq!(
        DomainError::from(AccessRefusal::MfaRequired),
        DomainError::MfaRequired
    );
}

#[test]
fn staff_for_access_predicate() {
    let mods = [role(10)];
    assert!(is_staff_for_access(
        &ActorFacts {
            is_owner: true,
            ..actor(&[], &[])
        },
        &mods
    ));
    assert!(is_staff_for_access(
        &actor(&[Permission::Administrator], &[]),
        &mods
    ));
    assert!(is_staff_for_access(
        &actor(&[Permission::ModerateMembers], &[]),
        &mods
    ));
    assert!(is_staff_for_access(&actor(&[], &[10]), &mods));
    assert!(!is_staff_for_access(
        &actor(&[Permission::ManageGuild], &[20]),
        &mods
    ));
}

#[test]
fn operation_table() {
    use Requirement as R;
    let cases = [
        (Operation::ApplyRule, R::Capability(Capability::Moderate)),
        (Operation::ApplyHoneypotRule, R::Quack),
        (Operation::ViewCases, R::Capability(Capability::Moderate)),
        (Operation::EditCase, R::Capability(Capability::Moderate)),
        (Operation::VoidCase, R::Capability(Capability::Moderate)),
        (Operation::RetryAction, R::Capability(Capability::Moderate)),
        (
            Operation::DismissAction,
            R::Capability(Capability::Moderate),
        ),
        (Operation::UndoAction, R::Capability(Capability::Moderate)),
        (Operation::DecideAppeal, R::Capability(Capability::Moderate)),
        (Operation::SubmitAppeal, R::MemberSelf),
        (
            Operation::ManageRules,
            R::Capability(Capability::ManageRules),
        ),
        (
            Operation::ReadSettings,
            R::Capability(Capability::ManageSettings),
        ),
        (
            Operation::ChangeSettings,
            R::Capability(Capability::ManageSettings),
        ),
        (
            Operation::ChangeModeratorRoles,
            R::Capability(Capability::ManageStaffRoles),
        ),
        (Operation::Setup, R::Capability(Capability::ManageSettings)),
        (
            Operation::ServerHealth,
            R::Capability(Capability::ManageSettings),
        ),
        (Operation::ReadModLog, R::Capability(Capability::Moderate)),
        (Operation::OpenOwnTicket, R::ServerMember),
        (Operation::CloseTicket, R::TicketMemberOrModerator),
        (
            Operation::FixTicketAccess,
            R::Capability(Capability::Moderate),
        ),
    ];
    for (op, req) in cases {
        assert_eq!(op.requirement(), req, "{op:?}");
    }
}
