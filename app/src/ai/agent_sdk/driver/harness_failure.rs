use std::fmt;

/// Only sanitized, bounded text crosses into the local error reporting path.
#[derive(Debug, Default)]
pub struct HarnessFailureOutput(String);

impl HarnessFailureOutput {
    pub(super) fn from_plaintext(
        mut text: String,
        known_secrets: &[&str],
        custom_patterns: &[regex::Regex],
    ) -> Self {
        // Scan the complete text before cutting it: a cut through a credential
        // would otherwise prevent its pattern from matching.
        let patterns: Vec<_> = secret_redaction::regexes::DEFAULT_REGEXES_WITH_NAMES
            .iter()
            .map(|entry| entry.pattern)
            .collect();
        let Ok(regex) = regex_automata::meta::Regex::new_many(&patterns) else {
            return Self::default();
        };
        let mut ranges: Vec<_> = regex.find_iter(&text).map(|hit| hit.range()).collect();
        for pattern in custom_patterns {
            ranges.extend(pattern.find_iter(&text).map(|hit| hit.range()));
        }
        for secret in known_secrets.iter().filter(|s| !s.is_empty()) {
            ranges.extend(
                text.match_indices(*secret)
                    .map(|(start, _)| start..start + secret.len()),
            );
        }
        ranges.sort_unstable_by_key(|range| range.start);
        let mut merged: Vec<std::ops::Range<usize>> = Vec::new();
        for range in ranges {
            if let Some(previous) = merged.last_mut()
                && range.start <= previous.end
            {
                previous.end = previous.end.max(range.end);
            } else {
                merged.push(range);
            }
        }
        for range in merged.into_iter().rev() {
            text.replace_range(range, "[REDACTED]");
        }
        let text = text.trim();
        const LIMIT: usize = 4096;
        const MARKER: &str = "\n… harness output truncated …\n";
        if text.len() <= LIMIT {
            return Self(text.to_owned());
        }
        let budget = LIMIT - MARKER.len();
        let mut head = budget / 2;
        let mut tail = text.len() - (budget - head);
        while !text.is_char_boundary(head) {
            head -= 1;
        }
        while !text.is_char_boundary(tail) {
            tail += 1;
        }
        Self(format!("{}{MARKER}{}", &text[..head], &text[tail..]))
    }
}

impl fmt::Display for HarnessFailureOutput {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.0.is_empty() {
            Ok(())
        } else {
            write!(formatter, "\n{}", self.0)
        }
    }
}

#[cfg(test)]
#[path = "harness_failure_tests.rs"]
mod tests;
