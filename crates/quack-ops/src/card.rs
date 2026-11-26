//! A message with its card image: the card is drawn on a blocking thread
//! (it takes tens of milliseconds) and attached under the name the
//! message's image block uses.

use quack_engine::port::OutgoingFile;
use quack_text::card::Card;
use quack_text::{Block, Msg, Text};

/// An ops answer with the files it shows (a card image, or none).
#[derive(Clone, Debug)]
pub struct Rendered {
    /// The message.
    pub msg: Msg,
    /// The files its image blocks name.
    pub files: Vec<OutgoingFile>,
}

impl Rendered {
    /// A message without files.
    pub fn plain(msg: Msg) -> Self {
        Self {
            msg,
            files: Vec::new(),
        }
    }
}

/// `msg` with `card` drawn as `name`. When drawing fails (a bug), the
/// image block becomes the card's title and description, so the message
/// still says what matters.
pub(crate) async fn attach(mut msg: Msg, card: Card, name: &str) -> Rendered {
    let alt = card.alt.clone();
    let title = card.title.clone();
    let drawn = tokio::task::spawn_blocking(move || quack_card::png(&card)).await;
    match drawn {
        Ok(Ok(bytes)) => Rendered {
            msg,
            files: vec![OutgoingFile {
                name: name.to_owned(),
                content_type: Some("image/png".to_owned()),
                bytes,
            }],
        },
        failed => {
            let error = match failed {
                Ok(Err(e)) => e.to_string(),
                Err(e) => e.to_string(),
                Ok(Ok(_)) => String::new(),
            };
            tracing::warn!(%error, card = name, "couldn't draw an ops card");
            for block in &mut msg.blocks {
                if matches!(block, Block::Image { name: n, .. } if n == name) {
                    *block = Block::Text(Text::new().bold(title.clone()).s(format!("\n{alt}")));
                }
            }
            Rendered::plain(msg)
        }
    }
}
