//! One insta snapshot per message id: every variant rendered for Discord
//! (after the budget trim), as plain text, and as the model.

use std::fmt::Write as _;

use super::fixtures::commands;
use super::samples::{Out, all};
use crate::budget;
use crate::model::{Block, ButtonTarget, InputKind, Modal, Msg, Row, SelectOptions, Text};
use crate::render;

/// The message as Discord would get it: trimmed to the budget.
pub fn fitted(m: &Msg) -> Msg {
    let mut m = m.clone();
    budget::fit(&mut m);
    m
}

fn controls(m: &Msg, out: &mut String) {
    for row in &m.rows {
        match row {
            Row::Buttons(buttons) => {
                let labels: Vec<String> = buttons
                    .iter()
                    .map(|b| match &b.target {
                        ButtonTarget::Link(url) => format!("[{}]({url})", b.label),
                        ButtonTarget::Action(a) => format!("[{} · {:?}] {a:?}", b.label, b.style),
                    })
                    .collect();
                let _ = writeln!(out, "buttons: {}", labels.join(" | "));
            }
            Row::Select(s) => {
                let options = match &s.options {
                    SelectOptions::TextChannels => "text channels".to_owned(),
                    SelectOptions::Strings(o) => o
                        .iter()
                        .map(|o| match &o.description {
                            Some(d) => format!("{} ({d})", o.label),
                            None => o.label.clone(),
                        })
                        .collect::<Vec<_>>()
                        .join(" | "),
                };
                let _ = writeln!(
                    out,
                    "select {:?} \"{}\": {options}",
                    s.action, s.placeholder
                );
            }
        }
    }
}

fn model_text(t: &Text) -> String {
    format!("{:?}", t.0)
}

fn model(m: &Msg, out: &mut String) {
    for b in &m.blocks {
        let _ = match b {
            Block::Heading(t) => writeln!(out, "Heading {}", model_text(t)),
            Block::Text(t) => writeln!(out, "Text {}", model_text(t)),
            Block::Quote(t) => writeln!(out, "Quote {}", model_text(t)),
            Block::Field { label, value } => writeln!(out, "Field {label:?} {}", model_text(value)),
            Block::Subtext(t) => writeln!(out, "Subtext {}", model_text(t)),
            Block::Divider => writeln!(out, "Divider"),
            Block::File(name) => writeln!(out, "File {name:?}"),
        };
    }
}

fn modal_text(m: &Modal, out: &mut String) {
    let _ = writeln!(out, "modal \"{}\" {:?}", m.title, m.action);
    for i in &m.inputs {
        let req = if i.required { "required" } else { "optional" };
        let _ = writeln!(out, "- {:?} \"{}\" ({req})", i.id, i.label);
        if let Some(d) = &i.description {
            let _ = writeln!(out, "  description: {d}");
        }
        let _ = match &i.kind {
            InputKind::Text {
                style,
                placeholder,
                min_length,
                max_length,
                value,
            } => writeln!(
                out,
                "  {style:?} text, {min_length}–{max_length} characters, placeholder {placeholder:?}, value {:?}",
                value.as_deref().map(|v| crate::fmt::cut(v, 80))
            ),
            InputKind::YesNo { placeholder, value } => {
                writeln!(
                    out,
                    "  Yes/No select, placeholder {placeholder:?}, value {value:?}"
                )
            }
        };
    }
}

/// Renders one output for a snapshot.
pub fn rendered(out: &Out) -> String {
    let ids = commands();
    let mut s = String::new();
    match out {
        Out::Msg(m) => {
            let f = fitted(m);
            let _ = writeln!(s, "tone: {:?}\n--- discord ---", f.tone);
            let _ = writeln!(s, "{}", render::msg_markdown(&f, &ids));
            controls(&f, &mut s);
            let _ = writeln!(s, "--- plain ---\n{}", m.plain());
            let _ = writeln!(s, "--- model ---");
            model(m, &mut s);
        }
        Out::Text(t) => {
            let _ = writeln!(s, "discord: {}", render::markdown(t, &ids));
            let _ = writeln!(s, "plain:   {}", t.plain());
            let _ = writeln!(s, "model:   {}", model_text(t));
        }
        Out::Modal(m) => modal_text(m, &mut s),
        Out::Str(text, _) => {
            let _ = writeln!(s, "{text}");
        }
    }
    s
}

#[test]
fn every_message() {
    let samples = all();
    let mut order: Vec<&'static str> = Vec::new();
    for s in &samples {
        if !order.contains(&s.id) {
            order.push(s.id);
        }
    }
    let mut settings = insta::Settings::clone_current();
    settings.set_prepend_module_to_snapshot(false);
    settings.set_omit_expression(true);
    settings.bind(|| {
        for id in order {
            let mut body = String::new();
            for s in samples.iter().filter(|s| s.id == id) {
                if !s.variant.is_empty() {
                    let _ = writeln!(body, "## {}", s.variant);
                }
                body.push_str(&rendered(&s.out));
                body.push('\n');
            }
            insta::assert_snapshot!(id, body);
        }
    });
}
