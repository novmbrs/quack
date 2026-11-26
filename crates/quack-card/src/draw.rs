//! Lays a [`Card`] out top to bottom: the header, each section, the
//! footer. Every section draws at `y` and returns the height it used.

use quack_text::Tone;
use quack_text::card::{Bar, Card, Check, Columns, Section, Tile};

use crate::svg::{Anchor, FAINT, Face, LINE, MUTED, SERIES, SURFACE, Svg, TEXT, fit, tone, width};

/// The image's width.
pub(crate) const WIDTH: f32 = 880.0;
/// The margin all around.
const PAD: f32 = 36.0;
/// The width inside the margins.
const INNER: f32 = WIDTH - 2.0 * PAD;
/// The space between sections.
const GAP: f32 = 26.0;
/// Corner radius of surfaces.
const RADIUS: f32 = 14.0;

/// The card as SVG.
pub(crate) fn card(card: &Card) -> String {
    let mut svg = Svg::default();
    let mut y = header(&mut svg, card);
    for section in &card.sections {
        y += GAP;
        y += match section {
            Section::Tiles(tiles) => draw_tiles(&mut svg, y, tiles),
            Section::Checks { title, rows } => checks(&mut svg, y, title.as_deref(), rows),
            Section::Columns(c) => columns(&mut svg, y, c),
            Section::Bars {
                title,
                rows,
                out_of,
            } => bars(&mut svg, y, title, rows, *out_of),
            Section::Table {
                title,
                header,
                rows,
            } => table(&mut svg, y, title, header, rows),
        };
    }
    y += GAP;
    y += footer(&mut svg, y, card);
    svg.finish(WIDTH, y)
}

/// Where the logo sits: x, y, and size, in logical pixels.
pub(crate) const LOGO: (f32, f32, f32) = (PAD - 2.0, PAD + 2.0, 48.0);

/// The stripe, the logo's place, the title and subtitle, and the badge.
fn header(svg: &mut Svg, card: &Card) -> f32 {
    let accent = tone(card.tone);
    svg.raw(&format!(
        r#"<rect x="0" y="0" width="{WIDTH}" height="5" fill="{accent}"/>"#
    ));
    let top = PAD + 4.0;
    // Quack's logo goes here, drawn onto the image after the SVG
    // (`LOGO`); a soft disc holds its place if it can't be drawn.
    svg.dot(
        LOGO.0 + LOGO.2 / 2.0,
        LOGO.1 + LOGO.2 / 2.0,
        LOGO.2 / 2.0,
        SURFACE,
    );

    let text_x = PAD + 60.0;
    let mut badge_w = 0.0;
    if let Some(badge) = &card.badge {
        badge_w = width(badge, 14.0, Face::Semibold) + 44.0;
        let x = WIDTH - PAD - badge_w;
        svg.wash(x, top + 4.0, badge_w, 34.0, 17.0, accent, 0.16);
        svg.dot(x + 18.0, top + 21.0, 4.5, accent);
        svg.text(
            x + 30.0,
            top + 26.0,
            badge,
            14.0,
            Face::Semibold,
            accent,
            Anchor::Start,
        );
    }
    let room = INNER - 60.0 - badge_w - 16.0;
    match &card.subtitle {
        Some(sub) => {
            svg.text(
                text_x,
                top + 18.0,
                &fit(&card.title, 24.0, Face::Bold, room),
                24.0,
                Face::Bold,
                TEXT,
                Anchor::Start,
            );
            svg.text(
                text_x,
                top + 41.0,
                &fit(sub, 14.0, Face::Regular, room),
                14.0,
                Face::Regular,
                MUTED,
                Anchor::Start,
            );
        }
        None => svg.text(
            text_x,
            top + 30.0,
            &fit(&card.title, 24.0, Face::Bold, room),
            24.0,
            Face::Bold,
            TEXT,
            Anchor::Start,
        ),
    }
    top + 44.0
}

/// A section's small title; returns its height.
fn title(svg: &mut Svg, y: f32, title: &str, note: Option<&str>) -> f32 {
    svg.label(PAD + 2.0, y + 12.0, title, MUTED, Anchor::Start);
    if let Some(note) = note {
        svg.text(
            WIDTH - PAD - 2.0,
            y + 12.0,
            note,
            13.0,
            Face::Regular,
            MUTED,
            Anchor::End,
        );
    }
    24.0
}

fn draw_tiles(svg: &mut Svg, y: f32, tiles: &[Tile]) -> f32 {
    const H: f32 = 98.0;
    const SPACE: f32 = 14.0;
    let mut used = 0.0;
    for (row, chunk) in tiles.chunks(4).enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let n = chunk.len() as f32;
        let w = (INNER - SPACE * (n - 1.0)) / n;
        #[allow(clippy::cast_precision_loss)]
        let ty = y + row as f32 * (H + SPACE);
        for (i, t) in chunk.iter().enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let x = PAD + i as f32 * (w + SPACE);
            svg.rect(x, ty, w, H, RADIUS, SURFACE, Some(LINE));
            let mut lx = x + 18.0;
            if t.tone != Tone::Neutral {
                svg.dot(x + 22.0, ty + 25.5, 4.0, tone(t.tone));
                lx += 13.0;
            }
            svg.label(lx, ty + 30.0, &t.label, MUTED, Anchor::Start);
            let size = if w < 190.0 { 26.0 } else { 30.0 };
            svg.text(
                x + 18.0,
                ty + 66.0,
                &fit(&t.value, size, Face::Bold, w - 36.0),
                size,
                Face::Bold,
                TEXT,
                Anchor::Start,
            );
            if let Some(note) = &t.note {
                svg.text(
                    x + 18.0,
                    ty + 85.0,
                    &fit(note, 12.5, Face::Regular, w - 36.0),
                    12.5,
                    Face::Regular,
                    MUTED,
                    Anchor::Start,
                );
            }
        }
        used = ty + H - y;
    }
    used
}

fn checks(svg: &mut Svg, y: f32, heading: Option<&str>, rows: &[Check]) -> f32 {
    const ROW: f32 = 50.0;
    let top = heading.map_or(0.0, |h| title(svg, y, h, None));
    #[allow(clippy::cast_precision_loss)]
    let h = ROW * rows.len() as f32;
    svg.rect(PAD, y + top, INNER, h, RADIUS, SURFACE, Some(LINE));
    for (i, r) in rows.iter().enumerate() {
        #[allow(clippy::cast_precision_loss)]
        let ry = y + top + i as f32 * ROW;
        if i > 0 {
            svg.hline(PAD + 18.0, ry, INNER - 36.0);
        }
        let color = tone(r.tone);
        let mid = ry + ROW / 2.0;
        svg.status_dot(PAD + 26.0, mid, color);
        let state_w = width(&r.state, 15.0, Face::Semibold);
        svg.text(
            WIDTH - PAD - 20.0,
            mid + 5.0,
            &r.state,
            15.0,
            Face::Semibold,
            color,
            Anchor::End,
        );
        let room = INNER - 64.0 - state_w - 24.0;
        let label = fit(&r.label, 16.0, Face::Semibold, room);
        svg.text(
            PAD + 46.0,
            mid + 5.5,
            &label,
            16.0,
            Face::Semibold,
            TEXT,
            Anchor::Start,
        );
        if let Some(detail) = &r.detail {
            let lw = width(&label, 16.0, Face::Semibold) + 12.0;
            let left = room - lw;
            if left > 40.0 {
                svg.text(
                    PAD + 46.0 + lw,
                    mid + 5.0,
                    &fit(detail, 14.0, Face::Regular, left),
                    14.0,
                    Face::Regular,
                    MUTED,
                    Anchor::Start,
                );
            }
        }
    }
    top + h
}

#[allow(clippy::cast_precision_loss)] // Counts far below 2^24.
fn columns(svg: &mut Svg, y: f32, c: &Columns) -> f32 {
    const CHART: f32 = 116.0;
    const INSET: f32 = 18.0;
    let top = title(svg, y, &c.title, c.note.as_deref());
    let box_h = CHART + 2.0 * INSET + 22.0;
    svg.rect(PAD, y + top, INNER, box_h, RADIUS, SURFACE, Some(LINE));
    let (x0, base) = (PAD + INSET, y + top + INSET + CHART);
    let w = INNER - 2.0 * INSET;
    let n = c.values.len().max(1) as f32;
    let unit = w / n;
    let gap = (unit * 0.24).clamp(1.5, 10.0);
    let col = unit - gap;
    let max = c.values.iter().copied().max().unwrap_or(0);
    // A faint gridline at the peak and halfway.
    for frac in [0.5, 1.0] {
        let gy = base - CHART * frac;
        svg.raw(&format!(
            r#"<rect x="{x0:.1}" y="{gy:.1}" width="{w:.1}" height="1" fill="{LINE}" fill-opacity="0.6"/>"#
        ));
    }
    for (i, &v) in c.values.iter().enumerate() {
        let x = x0 + i as f32 * unit + gap / 2.0;
        let r = (col / 2.0).min(3.0);
        if v == 0 || max == 0 {
            svg.rect(x, base - 2.0, col, 2.0, 1.0, FAINT, None);
            continue;
        }
        let h = (v as f32 / max as f32 * CHART).max(4.0);
        let last = i + 1 == c.values.len();
        svg.rect(x, base - h, col, h, r, SERIES, None);
        if !last {
            // Older periods a touch dimmer, so now stands out.
            svg.wash(x, base - h, col, h, r, "#141519", 0.22);
        }
        if let Some(failed) = c.failed.as_ref().and_then(|f| f.get(i)).copied()
            && failed > 0
        {
            let fh = (failed.min(v) as f32 / max as f32 * CHART).max(3.0);
            svg.rect(x, base - fh, col, fh, r, tone(Tone::Danger), None);
        }
    }
    svg.hline(x0, base, w);
    let ly = base + 22.0;
    svg.text(x0, ly, &c.start, 12.0, Face::Regular, MUTED, Anchor::Start);
    svg.text(x0 + w, ly, &c.end, 12.0, Face::Regular, MUTED, Anchor::End);
    top + box_h
}

#[allow(clippy::cast_precision_loss)] // Counts far below 2^24.
fn bars(svg: &mut Svg, y: f32, heading: &str, rows: &[Bar], out_of: Option<u64>) -> f32 {
    const ROW: f32 = 34.0;
    let top = title(svg, y, heading, None);
    let h = ROW * rows.len() as f32 + 16.0;
    svg.rect(PAD, y + top, INNER, h, RADIUS, SURFACE, Some(LINE));
    let label_w = rows
        .iter()
        .map(|r| width(&r.label, 14.5, Face::Regular))
        .fold(0.0, f32::max)
        .clamp(90.0, 260.0)
        + 24.0;
    let shown_w = rows
        .iter()
        .map(|r| width(&r.shown, 13.5, Face::Mono))
        .fold(0.0, f32::max)
        + 24.0;
    let track_x = PAD + 18.0 + label_w;
    let track_w = INNER - 36.0 - label_w - shown_w;
    let max = out_of.unwrap_or_else(|| rows.iter().map(|r| r.value).max().unwrap_or(0));
    for (i, r) in rows.iter().enumerate() {
        let mid = y + top + 8.0 + i as f32 * ROW + ROW / 2.0;
        svg.text(
            PAD + 18.0,
            mid + 5.0,
            &fit(&r.label, 14.5, Face::Regular, label_w - 12.0),
            14.5,
            Face::Regular,
            TEXT,
            Anchor::Start,
        );
        svg.wash(track_x, mid - 5.0, track_w, 10.0, 5.0, "#ffffff", 0.06);
        if r.value > 0 && max > 0 {
            let fw = (r.value.min(max) as f32 / max as f32 * track_w).max(10.0);
            svg.rect(track_x, mid - 5.0, fw, 10.0, 5.0, SERIES, None);
        }
        svg.text(
            WIDTH - PAD - 18.0,
            mid + 5.0,
            &r.shown,
            13.5,
            Face::Mono,
            MUTED,
            Anchor::End,
        );
    }
    top + h
}

#[allow(clippy::cast_precision_loss)] // A handful of columns.
fn table(svg: &mut Svg, y: f32, heading: &str, header: &[String], rows: &[Vec<String>]) -> f32 {
    const HEAD: f32 = 38.0;
    const ROW: f32 = 31.0;
    let top = title(svg, y, heading, None);
    let h = HEAD + ROW * rows.len() as f32 + 8.0;
    svg.rect(PAD, y + top, INNER, h, RADIUS, SURFACE, Some(LINE));
    let first_w = rows
        .iter()
        .filter_map(|r| r.first())
        .chain(header.first())
        .map(|s| width(s, 14.0, Face::Mono))
        .fold(0.0, f32::max)
        .clamp(120.0, 300.0)
        + 24.0;
    let others = header.len().saturating_sub(1).max(1) as f32;
    let col_w = (INNER - 36.0 - first_w) / others;
    let right = |c: usize| PAD + 18.0 + first_w + col_w * c as f32;
    let hy = y + top + 25.0;
    for (c, cell) in header.iter().enumerate() {
        if c == 0 {
            svg.label(PAD + 18.0, hy, cell, MUTED, Anchor::Start);
        } else {
            svg.label(right(c), hy, cell, MUTED, Anchor::End);
        }
    }
    svg.hline(PAD + 18.0, y + top + HEAD, INNER - 36.0);
    for (i, row) in rows.iter().enumerate() {
        let ry = y + top + HEAD + 4.0 + i as f32 * ROW;
        if i % 2 == 1 {
            svg.wash(PAD + 8.0, ry, INNER - 16.0, ROW, 8.0, "#ffffff", 0.025);
        }
        for (c, cell) in row.iter().enumerate() {
            if c == 0 {
                svg.text(
                    PAD + 18.0,
                    ry + 20.5,
                    &fit(cell, 14.0, Face::Regular, first_w - 12.0),
                    14.0,
                    Face::Regular,
                    TEXT,
                    Anchor::Start,
                );
            } else {
                svg.text(
                    right(c),
                    ry + 20.5,
                    cell,
                    14.0,
                    Face::Mono,
                    TEXT,
                    Anchor::End,
                );
            }
        }
    }
    top + h
}

fn footer(svg: &mut Svg, y: f32, card: &Card) -> f32 {
    svg.hline(PAD, y, INNER);
    let ty = y + 26.0;
    svg.text(
        PAD + 2.0,
        ty,
        &card.footer,
        13.0,
        Face::Regular,
        FAINT,
        Anchor::Start,
    );
    if let Some(right) = &card.footer_right {
        svg.text(
            WIDTH - PAD - 2.0,
            ty,
            right,
            13.0,
            Face::Regular,
            FAINT,
            Anchor::End,
        );
    }
    26.0 + PAD - 8.0
}
