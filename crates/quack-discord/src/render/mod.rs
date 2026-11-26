//! The neutral message model → Components V2 (§11.11).
//!
//! Everything sits in one Container whose accent follows the tone. Each run
//! of consecutive text blocks becomes one Text Display; dividers become
//! Separators, files File components, rows Action Rows. The message is
//! first fitted to Discord's text budget (`quack_text::budget::fit`); the
//! component budget (40, counted with nesting) holds by construction, and
//! rows past it are dropped as a last resort.

mod modal;

#[cfg(test)]
mod tests;

use quack_text::budget;
use quack_text::model::{
    Block, Button, ButtonStyle, ButtonTarget, Msg, Row, Select, SelectOptions, Tone,
};
use quack_text::render::{CommandIds, block_markdown};
use twilight_model::channel::ChannelType;
use twilight_model::channel::message::Component;
use twilight_model::channel::message::component::{
    ActionRow, Button as TwButton, ButtonStyle as TwButtonStyle, Container, FileDisplay,
    MediaGallery, MediaGalleryItem, SelectMenu, SelectMenuOption, SelectMenuType, Separator,
    TextDisplay, UnfurledMediaItem,
};

use crate::custom_id::{CustomId, encode_select_value};

pub use modal::{RenderedModal, render_modal};

/// A message ready to send: its top-level components. Send it with the
/// `IS_COMPONENTS_V2` flag and no `content` or embeds (the client does).
#[derive(Clone, Debug, PartialEq)]
pub struct RenderedMessage {
    /// The container, then nothing else (rows live inside the container).
    pub components: Vec<Component>,
}

/// The accent colour for a tone (§11.11).
pub const fn accent(tone: Tone) -> Option<u32> {
    match tone {
        Tone::Neutral => None,
        Tone::Success => Some(0x3B_A5_5C),
        Tone::Warning => Some(0xFA_A6_1A),
        Tone::Danger => Some(0xED_42_45),
    }
}

fn text_display(content: String) -> Component {
    Component::TextDisplay(TextDisplay { content, id: None })
}

/// The container's children for the text blocks.
fn blocks(msg: &Msg, commands: &CommandIds) -> Vec<Component> {
    let mut out = Vec::new();
    let mut run: Vec<String> = Vec::new();
    let flush = |run: &mut Vec<String>, out: &mut Vec<Component>| {
        if !run.is_empty() {
            out.push(text_display(run.join("\n")));
            run.clear();
        }
    };
    for block in &msg.blocks {
        match block {
            Block::Divider => {
                flush(&mut run, &mut out);
                out.push(Component::Separator(Separator {
                    divider: Some(true),
                    id: None,
                    spacing: None,
                }));
            }
            Block::File(name) => {
                flush(&mut run, &mut out);
                out.push(Component::File(FileDisplay {
                    file: UnfurledMediaItem {
                        content_type: None,
                        height: None,
                        proxy_url: None,
                        url: format!("attachment://{}", crate::rest::multipart::upload_name(name)),
                        width: None,
                    },
                    id: None,
                    spoiler: None,
                }));
            }
            Block::Image { name, alt } => {
                flush(&mut run, &mut out);
                out.push(Component::MediaGallery(MediaGallery {
                    id: None,
                    items: vec![MediaGalleryItem {
                        description: Some(alt.chars().take(1024).collect()),
                        media: UnfurledMediaItem {
                            content_type: None,
                            height: None,
                            proxy_url: None,
                            url: format!(
                                "attachment://{}",
                                crate::rest::multipart::upload_name(name)
                            ),
                            width: None,
                        },
                        spoiler: None,
                    }],
                }));
            }
            Block::Gallery(items) => {
                flush(&mut run, &mut out);
                let items: Vec<MediaGalleryItem> = items
                    .iter()
                    .filter_map(|i| {
                        Some(MediaGalleryItem {
                            description: i
                                .description
                                .as_deref()
                                .map(|d| d.chars().take(1024).collect()),
                            media: UnfurledMediaItem {
                                content_type: None,
                                height: None,
                                proxy_url: None,
                                url: quack_text::fmt::safe_url(&i.url)
                                    .filter(|u| u.starts_with("https://"))?,
                                width: None,
                            },
                            spoiler: i.spoiler.then_some(true),
                        })
                    })
                    .take(10)
                    .collect();
                if !items.is_empty() {
                    out.push(Component::MediaGallery(MediaGallery { id: None, items }));
                }
            }
            text => {
                if let Some(markdown) = block_markdown(text, commands) {
                    run.push(markdown);
                }
            }
        }
    }
    flush(&mut run, &mut out);
    out
}

const fn button_style(style: ButtonStyle) -> TwButtonStyle {
    match style {
        ButtonStyle::Primary => TwButtonStyle::Primary,
        ButtonStyle::Secondary => TwButtonStyle::Secondary,
        ButtonStyle::Success => TwButtonStyle::Success,
        ButtonStyle::Danger => TwButtonStyle::Danger,
    }
}

fn button(b: &Button) -> Component {
    let (style, custom_id, url) = match &b.target {
        ButtonTarget::Link(url) => (TwButtonStyle::Link, None, Some(url.clone())),
        ButtonTarget::Action(action) => (
            button_style(b.style),
            Some(CustomId::from(action).encode()),
            None,
        ),
    };
    Component::Button(TwButton {
        custom_id,
        disabled: false,
        emoji: None,
        id: None,
        label: Some(b.label.clone()),
        sku_id: None,
        style,
        url,
    })
}

fn select(s: &Select) -> Component {
    let (kind, options, channel_types) = match &s.options {
        SelectOptions::Strings(options) => (
            SelectMenuType::Text,
            Some(
                options
                    .iter()
                    .take(budget::SELECT_OPTIONS)
                    .map(|o| SelectMenuOption {
                        default: false,
                        description: o.description.clone(),
                        emoji: None,
                        label: o.label.clone(),
                        value: encode_select_value(o.value),
                    })
                    .collect(),
            ),
            None,
        ),
        SelectOptions::TextChannels => (
            SelectMenuType::Channel,
            None,
            Some(vec![ChannelType::GuildText]),
        ),
    };
    Component::SelectMenu(SelectMenu {
        channel_types,
        custom_id: CustomId::from(s.action).encode(),
        default_values: None,
        disabled: false,
        id: None,
        kind,
        max_values: None,
        min_values: None,
        options,
        placeholder: Some(s.placeholder.clone()),
        required: None,
    })
}

fn row(r: &Row) -> (Component, usize) {
    let components: Vec<Component> = match r {
        Row::Buttons(buttons) => buttons
            .iter()
            .take(budget::BUTTONS_PER_ROW)
            .map(button)
            .collect(),
        Row::Select(s) => vec![select(s)],
    };
    let count = 1 + components.len();
    (
        Component::ActionRow(ActionRow {
            components,
            id: None,
        }),
        count,
    )
}

fn text_chars(children: &[Component]) -> usize {
    children
        .iter()
        .map(|c| match c {
            Component::TextDisplay(t) => t.content.chars().count(),
            _ => 0,
        })
        .sum()
}

/// Renders a message within Discord's budgets. `commands` makes command
/// mentions clickable.
pub fn render(msg: &Msg, commands: &CommandIds) -> RenderedMessage {
    let mut msg = msg.clone();
    budget::fit(&mut msg);
    let mut children = blocks(&msg, commands);
    // Command mentions are longer than the `/name` the budget measured.
    if text_chars(&children) > budget::TEXT_BUDGET {
        children = blocks(&msg, &CommandIds::EMPTY);
    }
    let mut count = 1 + children.len();
    for r in &msg.rows {
        let (component, size) = row(r);
        if count + size > budget::COMPONENT_BUDGET {
            tracing::warn!("a message had more components than Discord allows; rows were dropped");
            break;
        }
        count += size;
        children.push(component);
    }
    if children.is_empty() {
        children.push(text_display("\u{200b}".to_owned()));
    }
    RenderedMessage {
        components: vec![Component::Container(Container {
            accent_color: accent(msg.tone).map(Some),
            components: children,
            id: None,
            spoiler: None,
        })],
    }
}

/// Counts components with nesting, as Discord does.
pub fn component_count(components: &[Component]) -> usize {
    components
        .iter()
        .map(|c| {
            1 + match c {
                Component::Container(c) => component_count(&c.components),
                Component::ActionRow(r) => component_count(&r.components),
                Component::Label(l) => component_count(std::slice::from_ref(&*l.component)),
                _ => 0,
            }
        })
        .sum()
}
