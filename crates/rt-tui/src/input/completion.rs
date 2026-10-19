//! Pure completion helpers shared by path and command input.
//!
//! The helpers in this module do not perform I/O, which keeps them easy to
//! test and lets [`crate::input::PathInput`] execute filesystem scans on a
//! worker thread when asynchronous completion is enabled.

/// Returns the longest common prefix of `values` measured on Unicode scalar
/// boundaries.
pub fn longest_common_prefix<'a, I>(values: I) -> String
where
    I: IntoIterator<Item = &'a str>,
{
    let mut iter = values.into_iter();
    let Some(first) = iter.next() else {
        return String::new();
    };

    let mut prefix = first.to_string();
    for value in iter {
        let mut len = 0;
        for (a, b) in prefix.chars().zip(value.chars()) {
            if a != b {
                break;
            }
            len += a.len_utf8();
        }
        prefix.truncate(len);
        if prefix.is_empty() {
            break;
        }
    }
    prefix
}

/// Returns command names beginning with `prefix`, sorted lexicographically.
pub fn command_matches<'a, I>(commands: I, prefix: &str) -> Vec<String>
where
    I: IntoIterator<Item = &'a str>,
{
    let mut matches: Vec<String> = commands
        .into_iter()
        .filter(|command| command.starts_with(prefix))
        .map(str::to_string)
        .collect();
    matches.sort();
    matches.dedup();
    matches
}

/// Computes the completion of `prefix` against `candidates`.
///
/// Returns `(replacement, match_count)`.  `replacement` is the longest common
/// prefix of all matching candidates.  `None` is returned when nothing
/// matches.
pub fn complete_prefix<'a, I>(candidates: I, prefix: &str) -> Option<(String, usize)>
where
    I: IntoIterator<Item = &'a str>,
{
    let matches: Vec<&str> = candidates
        .into_iter()
        .filter(|candidate| candidate.starts_with(prefix))
        .collect();
    if matches.is_empty() {
        return None;
    }

    let common = longest_common_prefix(matches.iter().copied());
    Some((common, matches.len()))
}
