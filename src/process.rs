//! A subprocess helper shared by every module that shells out to an
//! external CLI with no timeout of its own (`nix`, `gh`).

use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use crate::error::PaxError;

/// Runs `cmd` to completion, capturing its output like [`Command::output`],
/// but kills it and returns an error (built from `to_error`) instead of
/// blocking forever if it hasn't exited within `timeout`. Each caller
/// supplies its own `PaxError` variant (`Fetch` for `nix`, `Upload` for
/// `gh`) so a timeout reads as belonging to the operation that hit it,
/// rather than a generic one-size-fits-all message.
pub(crate) fn run_with_timeout(
    cmd: &mut Command,
    timeout: Duration,
    to_error: impl Fn(String) -> PaxError,
) -> Result<Output, PaxError> {
    let program = cmd.get_program().to_string_lossy().into_owned();
    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| to_error(e.to_string()))?;

    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_status)) => break,
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(to_error(format!(
                        "`{program}` timed out after {}s",
                        timeout.as_secs()
                    )));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(to_error(e.to_string())),
        }
    }
    child
        .wait_with_output()
        .map_err(|e| to_error(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_with_timeout_returns_normal_output_when_the_command_finishes_in_time() {
        let output = run_with_timeout(
            Command::new("echo").arg("hi"),
            Duration::from_secs(5),
            PaxError::Fetch,
        )
        .unwrap();
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "hi");
    }

    #[test]
    fn run_with_timeout_kills_and_errors_on_a_command_that_outlives_the_deadline() {
        let start = Instant::now();
        let result = run_with_timeout(
            Command::new("sleep").arg("30"),
            Duration::from_millis(300),
            PaxError::Fetch,
        );
        assert!(matches!(result, Err(PaxError::Fetch(_))));
        assert!(result.unwrap_err().to_string().contains("timed out"));
        // Proves the child was actually killed rather than just abandoned —
        // if `wait()` after `kill()` didn't work, this would block for the
        // full 30s `sleep` instead of returning right after the deadline.
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn the_supplied_error_constructor_is_used_for_every_failure_path() {
        // A nonexistent program fails at spawn(), the earliest error path —
        // proves `to_error` isn't only wired up for the timeout case.
        let result = run_with_timeout(
            &mut Command::new("pax-core-nonexistent-binary-xyz"),
            Duration::from_secs(5),
            PaxError::Upload,
        );
        assert!(matches!(result, Err(PaxError::Upload(_))));
    }
}
