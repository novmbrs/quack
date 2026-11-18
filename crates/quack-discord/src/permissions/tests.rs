#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "role indices in these tests are tiny"
)]

use proptest::prelude::*;
use quack_core::ids::{GuildId, MemberId};
use quack_core::permissions::RoleFacts;
use time::macros::datetime;

use super::*;

const GUILD: u64 = 100;
const OWNER: u64 = 1;
const ALICE: u64 = 2;

fn role(id: u64, position: u32, permissions: u64) -> RoleFacts {
    RoleFacts {
        id: RoleId::new(id).unwrap(),
        name: format!("role {id}"),
        position,
        permissions: Permissions(permissions),
        managed: false,
    }
}

fn guild(roles: Vec<RoleFacts>) -> GuildFacts {
    GuildFacts {
        guild_id: GuildId::new(GUILD).unwrap(),
        name: "Server".into(),
        icon_hash: None,
        owner_id: AccountId::new(OWNER).unwrap(),
        mfa_required: false,
        roles,
        quack: None,
    }
}

fn r(id: u64) -> RoleId {
    RoleId::new(id).unwrap()
}

fn who(account: u64, roles: &[RoleId]) -> Subject<'_> {
    Subject {
        account: AccountId::new(account).unwrap(),
        role_ids: roles,
        timed_out: false,
    }
}

fn ow(target: OverwriteTarget, allow: u64, deny: u64) -> Overwrite {
    Overwrite {
        target,
        allow: Permissions(allow),
        deny: Permissions(deny),
    }
}

const VIEW: u64 = VIEW_CHANNEL;
const SEND: u64 = SEND_MESSAGES;
const ATTACH: u64 = Permission::AttachFiles.bit();
const BAN: u64 = Permission::BanMembers.bit();
const KICK: u64 = Permission::KickMembers.bit();

#[test]
fn server_permissions_add_up_roles() {
    let g = guild(vec![role(GUILD, 0, VIEW), role(10, 1, KICK), role(11, 2, BAN)]);
    assert_eq!(in_guild(&g, who(ALICE, &[])).0, VIEW);
    assert_eq!(in_guild(&g, who(ALICE, &[r(10)])).0, VIEW | KICK);
    assert_eq!(in_guild(&g, who(ALICE, &[r(10), r(11)])).0, VIEW | KICK | BAN);
    // A role the server no longer has grants nothing.
    assert_eq!(in_guild(&g, who(ALICE, &[r(99)])).0, VIEW);
    assert_eq!(highest_position(&g, &[r(10), r(11), r(99)]), 2);
    assert_eq!(highest_position(&g, &[]), 0);
}

#[test]
fn owner_and_administrator_get_everything() {
    let g = guild(vec![role(GUILD, 0, 0), role(10, 1, ADMINISTRATOR)]);
    assert_eq!(in_guild(&g, who(OWNER, &[])), ALL);
    assert_eq!(in_guild(&g, who(ALICE, &[r(10)])), ALL);
    let deny_all = [ow(OverwriteTarget::Member(MemberId::new(ALICE).unwrap()), 0, u64::MAX)];
    assert_eq!(in_channel(&g, who(ALICE, &[r(10)]), &deny_all), ALL);
    assert_eq!(in_channel(&g, who(OWNER, &[]), &deny_all), ALL);
}

#[test]
fn overwrites_apply_in_discord_order() {
    let g = guild(vec![role(GUILD, 0, VIEW | SEND), role(10, 1, 0), role(11, 2, 0)]);
    let everyone = OverwriteTarget::Role(r(GUILD));
    let alice = OverwriteTarget::Member(MemberId::new(ALICE).unwrap());
    // @everyone denied, a role allows: the role wins.
    let o = [ow(everyone, 0, VIEW), ow(OverwriteTarget::Role(r(10)), VIEW, 0)];
    assert_eq!(in_channel(&g, who(ALICE, &[r(10)]), &o).0, VIEW | SEND);
    assert_eq!(in_channel(&g, who(ALICE, &[]), &o), Permissions::NONE);
    // One role allows, another denies: allow wins (allows are applied last).
    let o = [
        ow(OverwriteTarget::Role(r(10)), ATTACH, 0),
        ow(OverwriteTarget::Role(r(11)), 0, ATTACH),
    ];
    assert_eq!(in_channel(&g, who(ALICE, &[r(10), r(11)]), &o).0, VIEW | SEND | ATTACH);
    // The member's own overwrite beats every role.
    let o = [
        ow(OverwriteTarget::Role(r(10)), ATTACH, 0),
        ow(alice, 0, ATTACH),
    ];
    assert_eq!(in_channel(&g, who(ALICE, &[r(10)]), &o).0, VIEW | SEND);
}

#[test]
fn implicit_rules_follow_view_and_send() {
    let g = guild(vec![role(GUILD, 0, VIEW | SEND | ATTACH | BAN)]);
    let everyone = OverwriteTarget::Role(r(GUILD));
    assert_eq!(
        in_channel(&g, who(ALICE, &[]), &[ow(everyone, 0, VIEW)]),
        Permissions::NONE
    );
    assert_eq!(
        in_channel(&g, who(ALICE, &[]), &[ow(everyone, 0, SEND)]).0,
        VIEW | BAN
    );
}

#[test]
fn timeouts_leave_view_and_history() {
    let g = guild(vec![role(GUILD, 0, VIEW | SEND | READ_MESSAGE_HISTORY | KICK)]);
    let mut subject = who(ALICE, &[]);
    subject.timed_out = true;
    assert_eq!(in_guild(&g, subject).0, VIEW | READ_MESSAGE_HISTORY);
    let allow_send = [ow(OverwriteTarget::Role(r(GUILD)), SEND, 0)];
    assert_eq!(in_channel(&g, subject, &allow_send).0, VIEW | READ_MESSAGE_HISTORY);
}

#[test]
fn facts_from_members_and_channels() {
    let mut g = guild(vec![role(GUILD, 0, VIEW | SEND), role(10, 3, BAN)]);
    let quack = Member {
        id: MemberId::new(50).unwrap(),
        bot: true,
        username: "Quack".into(),
        global_name: None,
        avatar_hash: None,
        nick: None,
        role_ids: vec![r(10), r(99)],
        joined_at: None,
        timeout_until: Some(datetime!(2026-01-01 0:00 UTC)),
    };
    let now = datetime!(2026-06-01 0:00 UTC);
    let facts = member_facts(&g, &quack, now);
    assert_eq!(facts.role_ids, vec![r(10)]);
    assert_eq!(facts.highest_role_position, 3);
    assert_eq!(facts.permissions.0, VIEW | SEND | BAN);
    let earlier = member_facts(&g, &quack, datetime!(2025-06-01 0:00 UTC));
    assert_eq!(earlier.permissions.0, VIEW);
    g.quack = Some(facts);
    let parent = Channel {
        id: quack_core::ids::ChannelId::new(7).unwrap(),
        guild_id: Some(g.guild_id),
        kind: 0,
        name: "staff".into(),
        parent_id: None,
        overwrites: vec![ow(OverwriteTarget::Role(r(GUILD)), 0, SEND)],
        thread: None,
    };
    let thread = Channel {
        id: quack_core::ids::ChannelId::new(8).unwrap(),
        kind: 12,
        name: "ticket".into(),
        parent_id: Some(parent.id),
        overwrites: vec![],
        thread: Some(crate::model::ThreadState {
            archived: false,
            locked: false,
        }),
        ..parent.clone()
    };
    assert_eq!(channel_facts(&g, &parent, None, now).quack_permissions.0, VIEW | BAN);
    let in_thread = channel_facts(&g, &thread, Some(&parent), now);
    assert_eq!(in_thread.quack_permissions.0, VIEW | BAN);
    assert_eq!(in_thread.overwrites, parent.overwrites);
    assert_eq!(
        channel_facts(&g, &thread, None, now).quack_permissions.0,
        VIEW | SEND | BAN
    );
    g.quack = None;
    assert_eq!(
        channel_facts(&g, &parent, None, now).quack_permissions,
        Permissions::NONE
    );
}

/// Discord's precedence decided one bit at a time: the member overwrite if
/// it mentions the bit, else any role overwrite (allow beats deny), else
/// the @everyone overwrite, else the server permission.
fn oracle(base: u64, everyone: (u64, u64), roles: &[(u64, u64)], member: (u64, u64)) -> u64 {
    if base & ADMINISTRATOR != 0 {
        return u64::MAX;
    }
    let mut out = 0;
    for bit in (0..64).map(|i| 1_u64 << i) {
        let set = if member.0 & bit != 0 {
            true
        } else if member.1 & bit != 0 {
            false
        } else if roles.iter().any(|(a, _)| a & bit != 0) {
            true
        } else if roles.iter().any(|(_, d)| d & bit != 0) {
            false
        } else if everyone.0 & bit != 0 {
            true
        } else if everyone.1 & bit != 0 {
            false
        } else {
            base & bit != 0
        };
        if set {
            out |= bit;
        }
    }
    if out & VIEW == 0 {
        0
    } else if out & SEND == 0 {
        out & !NEEDS_SEND
    } else {
        out
    }
}

/// Bits that make the comparison interesting (Administrator is rare on
/// purpose so most cases exercise overwrites).
fn bits() -> impl Strategy<Value = u64> {
    let interesting = [VIEW, SEND, ATTACH, BAN, KICK, 1 << 17, READ_MESSAGE_HISTORY];
    proptest::collection::vec(proptest::sample::select(interesting.to_vec()), 0..5)
        .prop_map(|v| v.into_iter().fold(0, |a, b| a | b))
}

proptest! {
    #[test]
    fn channel_permissions_match_the_per_bit_oracle(
        everyone_role in bits(),
        role_perms in proptest::collection::vec(bits(), 3),
        admin in proptest::bool::weighted(0.05),
        everyone_ow in (bits(), bits()),
        role_ows in proptest::collection::vec((bits(), bits()), 3),
        member_ow in (bits(), bits()),
        held in proptest::collection::vec(any::<bool>(), 3),
    ) {
        let mut roles = vec![role(GUILD, 0, everyone_role)];
        for (i, p) in role_perms.iter().enumerate() {
            let extra = if admin && i == 0 { ADMINISTRATOR } else { 0 };
            roles.push(role(10 + i as u64, i as u32 + 1, p | extra));
        }
        let g = guild(roles);
        let mine: Vec<RoleId> = (0..3).filter(|i| held[*i]).map(|i| r(10 + i as u64)).collect();
        let mut overwrites = vec![ow(OverwriteTarget::Role(r(GUILD)), everyone_ow.0, everyone_ow.1)];
        for (i, (a, d)) in role_ows.iter().enumerate() {
            overwrites.push(ow(OverwriteTarget::Role(r(10 + i as u64)), *a, *d));
        }
        overwrites.push(ow(OverwriteTarget::Member(MemberId::new(ALICE).unwrap()), member_ow.0, member_ow.1));
        let base = in_guild(&g, who(ALICE, &mine)).0;
        let held_ows: Vec<(u64, u64)> = (0..3).filter(|i| held[*i]).map(|i| role_ows[i]).collect();
        let expected = oracle(base, everyone_ow, &held_ows, member_ow);
        prop_assert_eq!(in_channel(&g, who(ALICE, &mine), &overwrites).0, expected);
        // Overwrite order never matters.
        overwrites.reverse();
        prop_assert_eq!(in_channel(&g, who(ALICE, &mine), &overwrites).0, expected);
    }

    #[test]
    fn more_roles_never_take_server_permissions_away(
        perms in proptest::collection::vec(bits(), 4),
        held in proptest::collection::vec(any::<bool>(), 4),
    ) {
        let mut roles = vec![role(GUILD, 0, perms[0])];
        for (i, p) in perms.iter().enumerate().skip(1) {
            roles.push(role(10 + i as u64, i as u32, *p));
        }
        let g = guild(roles);
        let some: Vec<RoleId> = (1..4).filter(|i| held[*i]).map(|i| r(10 + i as u64)).collect();
        let all: Vec<RoleId> = (1..4).map(|i| r(10 + i as u64)).collect();
        let fewer = in_guild(&g, who(ALICE, &some));
        let more = in_guild(&g, who(ALICE, &all));
        prop_assert!(more.contains(fewer));
        prop_assert_eq!(in_guild(&g, who(OWNER, &some)), ALL);
    }
}
