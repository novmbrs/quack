//! DM channels and messages: create (with `nonce` + `enforce_nonce`, §10.4),
//! edit, delete, and read.

use quack_core::ids::{AccountId, ChannelId, MessageId};
use serde::Serialize;
use twilight_model::channel::message::{Component, MessageFlags};

use super::route::{Method, Route};
use super::transport::{Body, Request};
use super::{DiscordClient, Outcome, converted, json_body, multipart};
use crate::convert;
use crate::model::{FileEdit, Message, Nonce, OutgoingFile, SentMessage};
use crate::render::{RenderedMessage, render};
use quack_text::{CommandIds, Msg};

/// Who a message may ping. Mentions always render; they ping only when
/// listed here (§11.11: only the ticket welcome pings its member).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Mentions {
    /// Nobody (`allowed_mentions: {"parse": []}`).
    #[default]
    None,
    /// Exactly these accounts.
    Users(Vec<AccountId>),
}

/// What a message shows: a rendered Components V2 message, or nothing but
/// its files (evidence copies, §7.3).
#[derive(Clone, Debug, PartialEq)]
pub struct MessageBody {
    components: Option<Vec<Component>>,
    mentions: Mentions,
}

impl MessageBody {
    /// A Components V2 message.
    pub fn rendered(message: RenderedMessage) -> Self {
        Self {
            components: Some(message.components),
            mentions: Mentions::None,
        }
    }

    /// The body for a neutral message: rendered, or [`Self::files_only`]
    /// when it has no blocks and no rows (evidence copies, §7.3).
    pub fn from_msg(msg: &Msg, commands: &CommandIds) -> Self {
        if msg.blocks.is_empty() && msg.rows.is_empty() {
            Self::files_only()
        } else {
            Self::rendered(render(msg, commands))
        }
    }

    /// A plain message with only its files: no text, no components.
    pub const fn files_only() -> Self {
        Self {
            components: None,
            mentions: Mentions::None,
        }
    }

    /// The same message, allowed to ping these accounts.
    #[must_use]
    pub fn mentioning(mut self, users: Vec<AccountId>) -> Self {
        self.mentions = Mentions::Users(users);
        self
    }
}

#[derive(Serialize)]
pub(crate) struct AllowedMentions {
    parse: [&'static str; 0],
    #[serde(skip_serializing_if = "Vec::is_empty")]
    users: Vec<String>,
}

impl AllowedMentions {
    pub(crate) fn of(mentions: &Mentions) -> Self {
        Self {
            parse: [],
            users: match mentions {
                Mentions::None => Vec::new(),
                Mentions::Users(users) => users.iter().map(ToString::to_string).collect(),
            },
        }
    }
}

/// An attachment entry: a new upload by index, or an existing one by id.
#[derive(Serialize)]
#[serde(untagged)]
enum AttachmentRef {
    Upload { id: usize, filename: String },
    Existing { id: String },
}

#[derive(Serialize)]
struct Payload<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    components: Option<&'a [Component]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    flags: Option<u64>,
    allowed_mentions: AllowedMentions,
    #[serde(skip_serializing_if = "Option::is_none")]
    attachments: Option<Vec<AttachmentRef>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    nonce: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enforce_nonce: Option<bool>,
}

impl<'a> Payload<'a> {
    fn new(body: &'a MessageBody) -> Self {
        Self {
            components: body.components.as_deref(),
            flags: body
                .components
                .as_ref()
                .map(|_| MessageFlags::IS_COMPONENTS_V2.bits()),
            allowed_mentions: AllowedMentions::of(&body.mentions),
            attachments: None,
            nonce: None,
            enforce_nonce: None,
        }
    }
}

fn uploads(files: &[&OutgoingFile]) -> Vec<AttachmentRef> {
    files
        .iter()
        .enumerate()
        .map(|(id, f)| AttachmentRef::Upload {
            id,
            filename: f.name.clone(),
        })
        .collect()
}

/// A JSON body, or multipart when there are files.
fn body_with_files(payload: &Payload<'_>, files: &[&OutgoingFile]) -> Body {
    let json = json_body(payload);
    match (files.is_empty(), json) {
        (false, Body::Json(bytes)) => multipart::form(&multipart::boundary(), &bytes, files),
        (_, json) => json,
    }
}

fn sent(message: &twilight_model::channel::Message) -> Option<SentMessage> {
    Some(SentMessage {
        channel_id: convert::id(message.channel_id)?,
        message_id: convert::id(message.id)?,
        attachments: message
            .attachments
            .iter()
            .filter_map(|a| Some((a.filename.clone(), convert::id(a.id)?)))
            .collect(),
    })
}

impl DiscordClient {
    /// Opens (or returns) the DM channel with an account
    /// (`POST /users/@me/channels`, idempotent).
    pub async fn open_dm(&self, account: AccountId) -> Outcome<ChannelId> {
        #[derive(Serialize)]
        struct Recipient {
            recipient_id: String,
        }
        let body = Recipient {
            recipient_id: account.to_string(),
        };
        self.json::<twilight_model::channel::Channel>(super::with_json(Route::dm_channel(), &body))
            .await
            .and_then(|c| converted(convert::id(c.id)))
    }

    /// Creates a message. With a nonce, `enforce_nonce` is set, so a repeat
    /// inside Discord's window returns the first message instead of a second
    /// one (§10.4).
    pub async fn send(
        &self,
        channel: ChannelId,
        body: &MessageBody,
        files: &[OutgoingFile],
        nonce: Option<&Nonce>,
    ) -> Outcome<SentMessage> {
        let files: Vec<&OutgoingFile> = files.iter().collect();
        let mut payload = Payload::new(body);
        if !files.is_empty() {
            payload.attachments = Some(uploads(&files));
        }
        if let Some(nonce) = nonce {
            payload.nonce = Some(nonce.as_str());
            payload.enforce_nonce = Some(true);
        }
        let request = Request {
            body: body_with_files(&payload, &files),
            files: !files.is_empty(),
            ..Request::new(Route::messages(Method::Post, channel))
        };
        self.json::<twilight_model::channel::Message>(request)
            .await
            .and_then(|m| converted(sent(&m)))
    }

    /// Edits a message Quack posted (`PATCH`), keeping or replacing its file.
    pub async fn edit(
        &self,
        channel: ChannelId,
        message: MessageId,
        body: &MessageBody,
        files: &FileEdit,
    ) -> Outcome<SentMessage> {
        let mut payload = Payload::new(body);
        let uploads_now: Vec<&OutgoingFile> = match files {
            FileEdit::None => Vec::new(),
            FileEdit::Keep(id) => {
                payload.attachments = Some(vec![AttachmentRef::Existing { id: id.to_string() }]);
                Vec::new()
            }
            FileEdit::Replace(file) => {
                payload.attachments = Some(uploads(&[file]));
                vec![file]
            }
        };
        let request = Request {
            body: body_with_files(&payload, &uploads_now),
            files: !uploads_now.is_empty(),
            ..Request::new(Route::message(Method::Patch, channel, message))
        };
        self.json::<twilight_model::channel::Message>(request)
            .await
            .and_then(|m| converted(sent(&m)))
    }

    /// Deletes a message.
    pub async fn delete_message(&self, channel: ChannelId, message: MessageId) -> Outcome<()> {
        self.unit(Request::new(Route::message(
            Method::Delete,
            channel,
            message,
        )))
        .await
    }

    /// Reads one message (for evidence, §7.2). REST messages carry no
    /// server id: read the channel to learn it.
    pub async fn fetch_message(&self, channel: ChannelId, message: MessageId) -> Outcome<Message> {
        self.json::<twilight_model::channel::Message>(Request::new(Route::message(
            Method::Get,
            channel,
            message,
        )))
        .await
        .and_then(|m| converted(convert::message(&m)))
    }

    /// Reads up to `limit` (1–100) messages before `before`, newest first
    /// (ticket transcripts, §13.2.4).
    pub async fn fetch_history(
        &self,
        channel: ChannelId,
        before: Option<MessageId>,
        limit: u8,
    ) -> Outcome<Vec<Message>> {
        self.json::<Vec<twilight_model::channel::Message>>(Request::new(Route::history(
            channel,
            before,
            limit.clamp(1, 100),
        )))
        .await
        .map(|messages| messages.iter().filter_map(convert::message).collect())
    }
}
