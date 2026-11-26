//! A tiny SVG writer: the palette, text with the bundled fonts, and the
//! few shapes the cards use. Coordinates are logical pixels.

use std::fmt::Write as _;

use quack_text::Tone;

/// The page behind everything.
pub(crate) const BG: &str = "#141519";
/// Tiles, lists, and charts sit on this.
pub(crate) const SURFACE: &str = "#1c1e24";
/// Hairlines.
pub(crate) const LINE: &str = "#2a2d36";
/// Main text.
pub(crate) const TEXT: &str = "#f2f3f5";
/// Secondary text.
pub(crate) const MUTED: &str = "#9ba1ad";
/// Labels under charts and empty tracks.
pub(crate) const FAINT: &str = "#5f6571";
/// The one series color.
pub(crate) const SERIES: &str = "#7c8cff";

/// The color of a tone: green, amber, red, or the series color.
pub(crate) const fn tone(t: Tone) -> &'static str {
    match t {
        Tone::Success => "#2fbf71",
        Tone::Warning => "#f0b232",
        Tone::Danger => "#f04848",
        Tone::Neutral => SERIES,
    }
}

/// A font face of the bundled set.
#[derive(Clone, Copy)]
pub(crate) enum Face {
    Regular,
    Semibold,
    Bold,
    Mono,
}

impl Face {
    const fn attrs(self) -> &'static str {
        match self {
            Self::Regular => r#"font-family="Inter" font-weight="400""#,
            Self::Semibold => r#"font-family="Inter SemiBold" font-weight="600""#,
            Self::Bold => r#"font-family="Inter" font-weight="700""#,
            Self::Mono => r#"font-family="JetBrains Mono Medium" font-weight="500""#,
        }
    }

    /// Roughly how wide one character is, per pixel of font size.
    const fn advance(self) -> f32 {
        match self {
            Self::Regular => 0.55,
            Self::Semibold => 0.57,
            Self::Bold => 0.59,
            Self::Mono => 0.6,
        }
    }
}

/// Where text hangs from its x.
#[derive(Clone, Copy)]
pub(crate) enum Anchor {
    Start,
    End,
}

/// About how wide `s` is at `size` in `face`, for placing what follows it.
#[allow(clippy::cast_precision_loss)] // A few hundred characters at most.
pub(crate) fn width(s: &str, size: f32, face: Face) -> f32 {
    let n = s.chars().count() as f32;
    n * size * face.advance()
}

/// `s` cut to about `max` pixels, with an ellipsis when cut.
pub(crate) fn fit(s: &str, size: f32, face: Face, max: f32) -> String {
    if width(s, size, face) <= max {
        return s.to_owned();
    }
    let per = size * face.advance();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let keep = ((max / per).floor() as usize).saturating_sub(1);
    let mut out: String = s.chars().take(keep).collect();
    out.push('…');
    out
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            c => out.push(c),
        }
    }
    out
}

/// The SVG body being written.
#[derive(Default)]
pub(crate) struct Svg {
    out: String,
}

impl Svg {
    /// Text at a baseline.
    pub(crate) fn text(
        &mut self,
        x: f32,
        y: f32,
        s: &str,
        size: f32,
        face: Face,
        fill: &str,
        anchor: Anchor,
    ) {
        let anchor = match anchor {
            Anchor::Start => "start",
            Anchor::End => "end",
        };
        let _ = write!(
            self.out,
            r#"<text x="{x:.1}" y="{y:.1}" font-size="{size}" {} fill="{fill}" text-anchor="{anchor}">{}</text>"#,
            face.attrs(),
            escape(s)
        );
    }

    /// A small uppercase label with a little letter spacing.
    pub(crate) fn label(&mut self, x: f32, y: f32, s: &str, fill: &str, anchor: Anchor) {
        let anchor = match anchor {
            Anchor::Start => "start",
            Anchor::End => "end",
        };
        let _ = write!(
            self.out,
            r#"<text x="{x:.1}" y="{y:.1}" font-size="12" {} letter-spacing="1.1" fill="{fill}" text-anchor="{anchor}">{}</text>"#,
            Face::Semibold.attrs(),
            escape(&s.to_uppercase())
        );
    }

    /// A rectangle with rounded corners.
    pub(crate) fn rect(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        r: f32,
        fill: &str,
        stroke: Option<&str>,
    ) {
        let stroke = stroke.map_or_else(String::new, |s| {
            format!(r#" stroke="{s}" stroke-width="1""#)
        });
        let _ = write!(
            self.out,
            r#"<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" rx="{r:.1}" fill="{fill}"{stroke}/>"#,
        );
    }

    /// A rectangle, translucent.
    pub(crate) fn wash(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        r: f32,
        fill: &str,
        opacity: f32,
    ) {
        let _ = write!(
            self.out,
            r#"<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="{h:.1}" rx="{r:.1}" fill="{fill}" fill-opacity="{opacity}"/>"#,
        );
    }

    /// A filled circle.
    pub(crate) fn dot(&mut self, x: f32, y: f32, r: f32, fill: &str) {
        let _ = write!(
            self.out,
            r#"<circle cx="{x:.1}" cy="{y:.1}" r="{r:.1}" fill="{fill}"/>"#
        );
    }

    /// A dot with a soft ring, for states.
    pub(crate) fn status_dot(&mut self, x: f32, y: f32, fill: &str) {
        let _ = write!(
            self.out,
            r#"<circle cx="{x:.1}" cy="{y:.1}" r="8" fill="{fill}" fill-opacity="0.18"/>"#
        );
        self.dot(x, y, 4.5, fill);
    }

    /// A horizontal hairline.
    pub(crate) fn hline(&mut self, x: f32, y: f32, w: f32) {
        let _ = write!(
            self.out,
            r#"<rect x="{x:.1}" y="{y:.1}" width="{w:.1}" height="1" fill="{LINE}"/>"#
        );
    }

    /// Raw markup Quack wrote itself (the duck).
    pub(crate) fn raw(&mut self, s: &str) {
        self.out.push_str(s);
    }

    /// The document around the body.
    pub(crate) fn finish(self, w: f32, h: f32) -> String {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}"><clipPath id="card"><rect width="{w}" height="{h}" rx="22"/></clipPath><g clip-path="url(#card)"><rect width="{w}" height="{h}" fill="{BG}"/>{}</g></svg>"#,
            self.out
        )
    }
}
