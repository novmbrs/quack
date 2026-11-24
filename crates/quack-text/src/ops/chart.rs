//! Small text charts for the ops messages: bars, sparklines, and aligned
//! tables, drawn in a [`Block::Code`](crate::model::Block::Code) so the
//! columns line up. Kept narrow (about 40 columns) for phones.

/// A bar `width` cells wide filled to `value / max`, in eighths, padded
/// with light shade.
pub(super) fn bar(value: u64, max: u64, width: usize) -> String {
    const EIGHTHS: [char; 8] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    let cells = u64::try_from(width).unwrap_or(0);
    let eighths = if max == 0 {
        0
    } else {
        u64::try_from(u128::from(value.min(max)) * u128::from(cells) * 8 / u128::from(max))
            .unwrap_or(u64::MAX)
    };
    // At least a sliver for anything above zero.
    let eighths = if value > 0 { eighths.max(1) } else { 0 };
    let full = usize::try_from(eighths / 8).unwrap_or(width);
    let part = usize::try_from(eighths % 8).unwrap_or(0);
    let mut out = "█".repeat(full);
    let mut used = full;
    if part > 0 {
        out.push(EIGHTHS[part]);
        used += 1;
    }
    out.push_str(&"░".repeat(width.saturating_sub(used)));
    out
}

/// One character per value, scaled to the largest: `▁` for zero and at
/// least `▂` for anything else.
pub(super) fn spark(values: &[u64]) -> String {
    const LEVELS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let max = values.iter().copied().max().unwrap_or(0);
    values
        .iter()
        .map(|&v| {
            if v == 0 || max == 0 {
                LEVELS[0]
            } else {
                let level = (u128::from(v) * 7).div_ceil(u128::from(max));
                LEVELS[usize::try_from(level).unwrap_or(7).clamp(1, 7)]
            }
        })
        .collect()
}

/// Rows of cells as aligned lines: the first column left-aligned, the
/// rest right-aligned, two spaces between columns.
pub(super) fn table(rows: &[Vec<String>]) -> String {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|c| {
            rows.iter()
                .filter_map(|r| r.get(c))
                .map(|s| s.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    rows.iter()
        .map(|row| {
            let line = row
                .iter()
                .enumerate()
                .map(|(c, cell)| {
                    let pad = widths[c].saturating_sub(cell.chars().count());
                    if c == 0 {
                        format!("{cell}{}", " ".repeat(pad))
                    } else {
                        format!("{}{cell}", " ".repeat(pad))
                    }
                })
                .collect::<Vec<_>>()
                .join("  ");
            line.trim_end().to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bars_fill_in_eighths() {
        assert_eq!(bar(0, 10, 4), "░░░░");
        assert_eq!(bar(10, 10, 4), "████");
        assert_eq!(bar(5, 10, 4), "██░░");
        assert_eq!(bar(1, 1_000, 4), "▏░░░");
        assert_eq!(bar(3, 0, 4), "▏░░░");
        assert_eq!(bar(9, 16, 4).chars().count(), 4);
    }

    #[test]
    fn sparklines_scale_to_the_peak() {
        assert_eq!(spark(&[0, 1, 2, 4, 8]), "▁▂▃▅█");
        assert_eq!(spark(&[0, 0]), "▁▁");
        assert_eq!(spark(&[]), "");
    }

    #[test]
    fn tables_align() {
        let t = table(&[
            vec!["kind".into(), "runs".into()],
            vec!["send_dm".into(), "1,204".into()],
        ]);
        assert_eq!(t, "kind      runs\nsend_dm  1,204");
    }
}
