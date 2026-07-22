//! Shells out to the real `sed` binary — GNU sed specifically. This tool
//! only works with GNU sed because `wrap_script`'s instrumentation relies
//! on GNU-specific behaviour (bare-word labels/branches interacting with
//! `{}` blocks the way it does, and `N` at EOF autoprinting rather than
//! POSIX's "print nothing and exit"). BSD/macOS sed will not produce
//! correct results here.
//!
//! An earlier version of this ran sed once per truncated input prefix (line
//! 1 alone, then lines 1-2, etc.) to get "the real state up to line n"
//! without needing to parse sed's internal state ourselves. That approach
//! was abandoned: truncating the input makes every inspected line look like
//! the true last line of the stream, so any `$`-anchored address (`$!d`,
//! `$p`, ...) misbehaves — it only ever sees "yes, I'm on the last line".
//! Running the whole file exactly once, as done here, is both correct and
//! (for anything but tiny files) cheaper.

use anyhow::{Context, Result};
use std::io::Write;
use std::process::{Command, Stdio};

use crate::instrument::{parse_cycles, wrap_script, Cycle};

pub fn build_wrapped_script(user_script: &str) -> String {
    wrap_script(user_script)
}

/// Runs `sed -n <wrapped_script>` once over the *whole* real input and
/// returns one `Cycle` per external cycle, in order (see `instrument`'s
/// module docs for what "external cycle" means once `N`/`D` are involved).
pub fn run_full(wrapped_script: &str, all_lines: &[String]) -> Result<Vec<Cycle>> {
    let input = all_lines.join("\n") + "\n";

    let mut child = Command::new("sed")
        .arg("-n")
        .arg(wrapped_script)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("failed to spawn sed (is GNU sed installed?)")?;

    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .context("failed to write to sed stdin")?;

    let output = child
        .wait_with_output()
        .context("sed did not exit cleanly")?;
    if !output.status.success() {
        anyhow::bail!(
            "sed exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_cycles(&stdout))
}
