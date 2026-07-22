use anyhow::{Context, Result};
use std::io::Write;
use std::process::{Command, Stdio};

use crate::instrument::{parse_cycles, wrap_script, Cycle};

pub fn build_wrapped_script(user_script: &str) -> String {
    wrap_script(user_script)
}

/// Runs `sed -n <wrapped_script>` once over the *whole* real input and
/// returns one `Cycle` per input line, in order. Running the full file (and
/// never a truncated prefix) is what keeps `$`-anchored addressing correct —
/// truncating would make every inspected line falsely look like the last
/// line of the stream.
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

    let output = child.wait_with_output().context("sed did not exit cleanly")?;
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
