//! The store snapshot's grammar and the diff between two of them. The
//! snapshot file is plain text: one store path per line, which is also
//! the shape the previous pipeline wrote, so old composite-run snapshots
//! still read.

use cachet_core::constants::PUSH_PATHS_MAX;

/// One store path per line, whitespace tolerant, empties dropped.
pub fn parse_snapshot(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// The paths the job added: `after` minus `before`, preserving after
/// order and collapsing repeats. A missing `before` degrades to the whole
/// store, and [`bound_candidates`] is what keeps that honest.
pub fn store_diff(before: &str, after: &str) -> Vec<String> {
    let mut seen: std::collections::BTreeSet<String> = parse_snapshot(before).into_iter().collect();
    parse_snapshot(after)
        .into_iter()
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

/// The candidate bound: past it the push refuses rather than uploading
/// the world on a misread snapshot.
///
/// # Errors
///
/// [`crate::PushError::TooManyCandidates`] past the cap.
pub fn bound_candidates(candidates: &[String]) -> Result<(), crate::PushError> {
    if candidates.len() as u64 > PUSH_PATHS_MAX {
        return Err(crate::PushError::TooManyCandidates(candidates.len()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grammar_is_lines_trimmed() {
        assert_eq!(
            parse_snapshot(" /nix/store/aaa-bash \n\n/nix/store/bbb-zsh\t\n"),
            vec![
                "/nix/store/aaa-bash".to_string(),
                "/nix/store/bbb-zsh".to_string(),
            ],
        );
        assert!(parse_snapshot("\n \n").is_empty());
    }

    #[test]
    fn the_diff_tracks_additions_in_order() {
        let before = "/nix/store/aaa-a\n/nix/store/bbb-b\n";
        let after = "/nix/store/bbb-b\n/nix/store/ccc-c\n/nix/store/ccc-c\n/nix/store/ddd-d\n";
        assert_eq!(
            store_diff(before, after),
            vec![
                "/nix/store/ccc-c".to_string(),
                "/nix/store/ddd-d".to_string(),
            ],
        );
        assert!(store_diff(before, before).is_empty());
    }

    #[test]
    fn the_bound_refuses_the_world() {
        let many: Vec<String> = (0..=PUSH_PATHS_MAX)
            .map(|i| format!("/nix/store/{i:032}-x"))
            .collect();
        assert!(matches!(
            bound_candidates(&many),
            Err(crate::PushError::TooManyCandidates(n)) if n as u64 == PUSH_PATHS_MAX + 1,
        ));
        let ok: Vec<String> = vec!["/nix/store/aaa-a".to_string()];
        assert!(bound_candidates(&ok).is_ok());
    }
}
