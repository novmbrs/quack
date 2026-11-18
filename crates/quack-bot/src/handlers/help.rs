//! `/help [topic]` (§12.12): static text, answered at once.

use quack_text::help::{Topic, help};

use crate::ack::{Finish, Pending};

/// Answers with the help text immediately (type 4, ephemeral).
pub(crate) async fn run(p: Pending<'_>, topic: Option<Topic>) -> Finish {
    let msg = help(topic, p.public_url());
    p.reply(&msg, Finish::Done).await
}
