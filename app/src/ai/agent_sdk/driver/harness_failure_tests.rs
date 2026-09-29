use super::HarnessFailureOutput;
use crate::ai::agent_sdk::driver::AgentDriverError;

fn message(text: &str, secrets: &[&str]) -> String {
    AgentDriverError::HarnessCommandFailed {
        exit_code: 7,
        output: HarnessFailureOutput::from_plaintext(text.to_owned(), secrets, &[]),
    }
    .to_string()
}

#[test]
fn harness_failure_shows_local_diagnostic_and_exit_code() {
    assert_eq!(
        message("  model file missing\n", &[]),
        "Harness command exited with code 7\nmodel file missing"
    );
    assert_eq!(message(" \n\t", &[]), "Harness command exited with code 7");
}

#[cfg(unix)]
#[test]
fn harness_exit_137_explains_sigkill_as_a_possible_oom_cause() {
    let error = AgentDriverError::HarnessCommandFailed {
        exit_code: 137,
        output: HarnessFailureOutput::from_plaintext("worker stopped".to_string(), &[], &[])
            .with_exit_code_hint(137),
    };

    let message = error.to_string();
    assert!(message.contains("may have been killed by SIGKILL"));
    assert!(message.contains("out-of-memory termination is one possible cause"));
    assert!(!message.contains("was killed by the OOM killer"));
}

#[cfg(unix)]
#[test]
fn other_harness_exit_codes_do_not_claim_memory_pressure() {
    let output = HarnessFailureOutput::from_plaintext("worker stopped".to_string(), &[], &[])
        .with_exit_code_hint(143);

    assert_eq!(output.to_string(), "\nworker stopped");
}

#[test]
fn harness_failure_bounds_output_without_losing_start_or_end() {
    for size in [4095, 4096, 4097, 10000] {
        let text = format!("START{}END", "x".repeat(size - 8));
        let output = HarnessFailureOutput::from_plaintext(text.clone(), &[], &[]).to_string();
        assert!(output.len() <= 4097);
        assert!(output.starts_with("\nSTART"));
        assert!(output.ends_with("END"));
        if size <= 4096 {
            assert_eq!(output, format!("\n{text}"));
        } else {
            assert!(output.contains("harness output truncated"));
        }
    }
}

#[test]
fn harness_failure_preserves_unicode_at_both_truncation_boundaries() {
    let text = format!("начало{}конец", "🙂".repeat(2000));
    let output = HarnessFailureOutput::from_plaintext(text, &[], &[]).to_string();
    assert!(output.len() <= 4097);
    assert!(output.starts_with("\nначало"));
    assert!(output.ends_with("конец"));
}

#[test]
fn harness_failure_redacts_before_truncation_and_in_debug_output() {
    let synthetic_key = format!("AKIA{}", "X".repeat(16));
    let text = format!(
        "{} {synthetic_key} {} local-test-credential",
        "a".repeat(2020),
        "b".repeat(5000)
    );
    let output = HarnessFailureOutput::from_plaintext(text, &["local-test-credential"], &[]);
    for formatted in [output.to_string(), format!("{output:?}")] {
        assert!(!formatted.contains("AKIA"));
        assert!(!formatted.contains("local-test-credential"));
        assert!(formatted.contains("[REDACTED]"));
    }
}

#[test]
fn harness_failure_redacts_overlapping_known_values_without_erasing_output() {
    let output = message(
        "failure: synthetic-long-value; synthetic",
        &["", "synthetic", "synthetic-long-value"],
    );
    assert_eq!(
        output,
        "Harness command exited with code 7\nfailure: [REDACTED]; [REDACTED]"
    );
}

#[test]
fn harness_failure_masks_known_credentials_before_partial_pattern_matches() {
    let synthetic = format!("sk-{}-private-suffix", "x".repeat(120));
    let output = message(&format!("failed: {synthetic}"), &[&synthetic]);
    assert_eq!(
        output,
        "Harness command exited with code 7\nfailed: [REDACTED]"
    );
}

#[test]
fn harness_failure_custom_patterns_apply_without_visual_safe_mode() {
    let output = HarnessFailureOutput::from_plaintext(
        "failed: private-prefix-sk-syntheticvalue-private-suffix".to_owned(),
        &["syntheticvalue"],
        &[regex::Regex::new("private-prefix-.*-private-suffix").unwrap()],
    );
    assert_eq!(output.to_string(), "\nfailed: [REDACTED]");
}

#[test]
fn harness_failure_missing_terminal_block_keeps_exit_code_and_local_hint() {
    use crate::ai::agent_sdk::driver::{AgentDriver, terminal::TerminalDriver};
    use crate::terminal::model::BlockId;
    use crate::test_util::terminal::{add_window_with_terminal, initialize_app_for_terminal_view};

    warpui::App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let terminal_view = add_window_with_terminal(&mut app, None);
        let driver = app.add_model(|ctx| {
            let terminal = TerminalDriver::create_from_existing_view(terminal_view, ctx);
            AgentDriver::new_for_test(std::env::temp_dir(), terminal, ctx)
        });
        let (tx, rx) = futures::channel::oneshot::channel();
        driver.update(&mut app, |_, ctx| {
            let foreground = ctx.spawner();
            ctx.spawn(
                async move {
                    let output =
                        AgentDriver::fetch_harness_failure_output(&BlockId::new(), &foreground, 7)
                            .await;
                    let ordinary_failure = AgentDriverError::HarnessCommandFailed {
                        exit_code: 7,
                        output,
                    }
                    .to_string();
                    let oom_output = AgentDriver::fetch_harness_failure_output(
                        &BlockId::new(),
                        &foreground,
                        137,
                    )
                    .await;
                    let possible_oom = AgentDriverError::HarnessCommandFailed {
                        exit_code: 137,
                        output: oom_output,
                    }
                    .to_string();
                    let _ = tx.send((ordinary_failure, possible_oom));
                },
                |_, _, _| {},
            );
        });
        let (ordinary_failure, possible_oom) = rx.await.unwrap();
        assert_eq!(ordinary_failure, "Harness command exited with code 7");
        if cfg!(unix) {
            assert_eq!(
                possible_oom,
                "Harness command exited with code 137\nThe process may have been killed by SIGKILL; out-of-memory termination is one possible cause."
            );
        } else {
            assert_eq!(possible_oom, "Harness command exited with code 137");
        }
    });
}
