//! Aligned text tables for the ops messages, drawn in a
//! [`Block::Code`](crate::model::Block::Code) so the columns line up. Kept
//! narrow (about 40 columns) for phones; charts are cards now.

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
    fn tables_align() {
        let t = table(&[
            vec!["kind".into(), "runs".into()],
            vec!["send_dm".into(), "1,204".into()],
        ]);
        assert_eq!(t, "kind      runs\nsend_dm  1,204");
    }
}
