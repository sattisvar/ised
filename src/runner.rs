//! Runs the wrapped script through `sed_rs`, a GNU-compatible sed
//! implementation in Rust, instead of shelling out to a system `sed`
//! binary. `wrap_script`'s instrumentation relies on GNU-specific
//! behaviour (bare-word labels/branches interacting with `{}` blocks the
//! way they do, and `N` at EOF autoprinting rather than POSIX's "print
//! nothing and exit"), which `sed_rs` implements — so no external GNU
//! sed installation is required.
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
use sed_rs::Sed;

use crate::instrument::{parse_cycles, wrap_script, Cycle};

pub fn build_wrapped_script(user_script: &str) -> String {
    wrap_script(user_script)
}

/// Runs the wrapped script in quiet mode (the equivalent of `sed -n`) once
/// over the *whole* real input and returns one `Cycle` per external cycle,
/// in order (see `instrument`'s module docs for what "external cycle" means
/// once `N`/`D` are involved).
pub fn run_full(wrapped_script: &str, all_lines: &[String]) -> Result<Vec<Cycle>> {
    let input = all_lines.join("\n") + "\n";

    let stdout = Sed::new(wrapped_script)
        .context("failed to parse sed script")?
        .quiet(true)
        .eval(&input)
        .context("sed script failed")?;

    Ok(parse_cycles(&stdout))
}
