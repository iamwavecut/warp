use chrono::{DateTime, Local};
use fuzzy_match::FuzzyMatchResult;
use ordered_float::OrderedFloat;

use crate::terminal::HistoryEntry;
use crate::terminal::model::session::SessionId;

const PRIOR_MULTIPLIER_BASELINE: f64 = 0.8;
const PRIOR_MULTIPLIER_SWING: f64 = 0.4;
const RECENCY_WEIGHT: f64 = 0.10;
const SESSION_WEIGHT: f64 = 0.05;
const EXIT_PENALTY_WEIGHT: f64 = 0.03;
const RECENCY_HALF_LIFE_DAYS: f64 = 3.0;
const RAW_SKIM_FLOOR_PER_CHAR: f64 = 8.0;
const CONSECUTIVE_BONUS_PER_CHAR: f64 = 4.0;
const EXACT_WHOLE_LINE_BONUS: f64 = 12.0;
const MISSING_TIMESTAMP_RECENCY: f64 = 0.5;
const PRIOR_SUM_MIN: f64 = -EXIT_PENALTY_WEIGHT;
const PRIOR_SUM_MAX: f64 = RECENCY_WEIGHT + SESSION_WEIGHT;

pub(crate) struct MatchQuality {
    adjusted_skim: f64,
    adjusted_skim_per_char: f64,
}

pub(crate) fn tokenize_query(query: &str) -> Vec<&str> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        vec![trimmed]
    } else {
        trimmed.split_whitespace().collect()
    }
}

pub(crate) fn match_history_command(
    command: &str,
    tokens: &[&str],
) -> Option<(FuzzyMatchResult, MatchQuality)> {
    let token_matches: Vec<FuzzyMatchResult> = tokens
        .iter()
        .map(|token| fuzzy_match::match_indices_case_insensitive(command, token))
        .collect::<Option<_>>()?;

    let mut merged_indices: Vec<usize> = token_matches
        .iter()
        .flat_map(|matched| matched.matched_indices.iter().copied())
        .collect();
    merged_indices.sort_unstable();
    merged_indices.dedup();

    let raw_score_total = token_matches.iter().map(|matched| matched.score).sum();
    let adjusted_skim = adjusted_skim(command, tokens, &token_matches);
    let query_char_count: usize = tokens.iter().map(|token| token.chars().count()).sum();
    let adjusted_skim_per_char = if query_char_count == 0 {
        0.0
    } else {
        adjusted_skim / query_char_count as f64
    };

    Some((
        FuzzyMatchResult {
            score: raw_score_total,
            matched_indices: merged_indices,
        },
        MatchQuality {
            adjusted_skim,
            adjusted_skim_per_char,
        },
    ))
}

fn adjusted_skim(command: &str, tokens: &[&str], token_matches: &[FuzzyMatchResult]) -> f64 {
    let token_score: f64 = token_matches
        .iter()
        .map(|matched| {
            matched.score as f64
                + longest_consecutive_run(&matched.matched_indices).saturating_sub(1) as f64
                    * CONSECUTIVE_BONUS_PER_CHAR
        })
        .sum();
    let query = tokens.join(" ");
    token_score
        + if !query.is_empty() && command.eq_ignore_ascii_case(&query) {
            EXACT_WHOLE_LINE_BONUS
        } else {
            0.0
        }
}

fn longest_consecutive_run(indices: &[usize]) -> usize {
    let mut longest = 0;
    let mut current = 0;
    let mut previous = None;
    for &index in indices {
        current = if previous == index.checked_sub(1) {
            current + 1
        } else {
            1
        };
        longest = longest.max(current);
        previous = Some(index);
    }
    longest
}

pub(crate) fn rank(
    entry: &HistoryEntry,
    match_quality: MatchQuality,
    now: DateTime<Local>,
    current_session_id: SessionId,
    is_blank_query: bool,
) -> Option<OrderedFloat<f64>> {
    if is_blank_query {
        return Some(OrderedFloat(0.0));
    }
    if match_quality.adjusted_skim_per_char < RAW_SKIM_FLOOR_PER_CHAR {
        return None;
    }

    let recency = entry
        .start_ts
        .map_or(MISSING_TIMESTAMP_RECENCY, |start_ts| {
            let age_days = ((now - start_ts).num_seconds() as f64
                / chrono::TimeDelta::days(1).num_seconds() as f64)
                .max(0.0);
            (-std::f64::consts::LN_2 * age_days / RECENCY_HALF_LIFE_DAYS).exp()
        });
    let session = f64::from(entry.session_id == Some(current_session_id));
    let exit_penalty = f64::from(entry.exit_code.is_some_and(|code| !code.was_successful()));
    let prior_sum =
        RECENCY_WEIGHT * recency + SESSION_WEIGHT * session - EXIT_PENALTY_WEIGHT * exit_penalty;
    let normalized_priors =
        ((prior_sum - PRIOR_SUM_MIN) / (PRIOR_SUM_MAX - PRIOR_SUM_MIN)).clamp(0.0, 1.0);
    let multiplier = PRIOR_MULTIPLIER_BASELINE + PRIOR_MULTIPLIER_SWING * normalized_priors;

    Some(OrderedFloat(match_quality.adjusted_skim * multiplier))
}
