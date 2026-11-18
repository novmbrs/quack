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
        let script = self.next_script(&recorded);
        recorded.scripted = script.as_ref().map(|s| format!("{s:?}"));
        self.record(recorded);
        match script {
            None | Some(Script::Pass) => self.world(effect),
            Some(Script::Refuse(r)) => Outcome::Refused(r),
            Some(Script::RetryAfter(d)) => Outcome::RetryAfter(d),
            Some(Script::NotSent) => Outcome::NotSent(NotSentCause::Connection),
            // Only moderation calls can land late ([`Self::mutation`]);
            // elsewhere a late change is simply lost.
            Some(Script::Unknown | Script::Late) => Outcome::Unknown(UnknownCause::Timeout),
            Some(Script::LoseResponse) => {
                let _ = self.world(effect);
                Outcome::Unknown(UnknownCause::Connection)
            }
            Some(Script::Hold(gate)) => {
                gate.hold().await;
                self.world(effect)
            }
            Some(Script::HoldAfter(gate) | Script::HoldAfterIn(gate, _)) => {
                let outcome = self.world(effect);
                gate.hold().await;
                outcome
            }
        }
    }

    /// [`Self::call`] for a moderation call, which can also land late
    /// ([`Script::Late`]).
    pub(super) async fn mutation(
        &self,
        mut recorded: Recorded,
        effect: impl FnOnce(&mut World) -> Outcome<()> + Send + 'static,
    ) -> Outcome<()> {
        let late = {
            let mut scripts = super::lock(&self.scripts);
            let queue = scripts.get_mut(&recorded.call);
            if queue
                .as_ref()
                .and_then(|q| q.front())
                .is_some_and(|s| matches!(s, Script::Late))
            {
                queue.and_then(std::collections::VecDeque::pop_front)
            } else {
                None
            }
        };
        let about = recorded.guild.zip(recorded.member);
        if late.is_none() {
            // An earlier request about this member lands before this one.
            let earlier: Vec<super::LateEffect> = {
                let mut pending = super::lock(&self.late);
                let (now, later) = std::mem::take(&mut *pending)
                    .into_iter()
                    .partition(|e| about.is_some() && e.0 == about);
                *pending = later;
                now
            };
            for e in earlier {
                self.world(|w| (e.1)(w));
            }
            return self.call(recorded, effect).await;
        }
        recorded.scripted = Some("Late".to_owned());
        self.record(recorded);
        super::lock(&self.late).push(super::LateEffect(
            about,
            Box::new(move |w| {
                let _ = effect(w);
            }),
        ));
        Outcome::Unknown(UnknownCause::Timeout)
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
