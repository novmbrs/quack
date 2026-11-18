//! Running one fake call: record it, apply its script, and apply its
//! effect to the world unless the script stops it.

use quack_core::ids::{GuildId, MemberId};
use quack_engine::port::{NotSentCause, Outcome, Refusal, RefusalKind, UnknownCause};

use super::state::World;
use super::{Call, FakeDiscord, Recorded, Script};

impl FakeDiscord {
    /// Runs one call: records it, applies its script, and (unless the
    /// script stops it) applies `effect` to the world.
    pub(super) async fn call<T: Send>(
        &self,
        mut recorded: Recorded,
        effect: impl FnOnce(&mut World) -> Outcome<T> + Send,
    ) -> Outcome<T> {
        let script = self.next_script(recorded.call);
        recorded.scripted = script.as_ref().map(|s| format!("{s:?}"));
        self.record(recorded);
        match script {
            None | Some(Script::Pass) => self.world(effect),
            Some(Script::Refuse(r)) => Outcome::Refused(r),
            Some(Script::RetryAfter(d)) => Outcome::RetryAfter(d),
            Some(Script::NotSent) => Outcome::NotSent(NotSentCause::Connection),
            Some(Script::Unknown) => Outcome::Unknown(UnknownCause::Timeout),
            Some(Script::LoseResponse) => {
                let _ = self.world(effect);
                Outcome::Unknown(UnknownCause::Connection)
            }
            Some(Script::Hold(gate)) => {
                gate.hold().await;
                self.world(effect)
            }
            Some(Script::HoldAfter(gate)) => {
                let outcome = self.world(effect);
                gate.hold().await;
                outcome
            }
        }
    }
}

/// Discord refused with `kind`.
pub(super) fn refused<T>(kind: RefusalKind) -> Outcome<T> {
    Outcome::Refused(Refusal::of(kind))
}

/// A recorded call, filled in by `f`.
pub(super) fn with(call: Call, f: impl FnOnce(&mut Recorded)) -> Recorded {
    let mut r = Recorded::new(call);
    f(&mut r);
    r
}

/// A recorded moderation call about `member`.
pub(super) fn about(call: Call, guild: GuildId, member: MemberId, reason: &str) -> Recorded {
    with(call, |r| {
        r.guild = Some(guild);
        r.member = Some(member);
        r.reason = Some(reason.to_owned());
    })
}
