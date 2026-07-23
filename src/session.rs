use anyhow::Result;

use crate::runner;
use crate::state::Block;

/// Drives sed once over the whole file (a single real invocation — running
/// only a truncated prefix breaks `$`-anchored addressing), then groups the
/// resulting cycles into `Block`s: each block is one or more consecutive
/// raw input lines that a single external cycle consumed together (more
/// than one line only when the script uses `N`).
///
/// Computation is lazy (triggered by the first call to `get`/`block_count`,
/// not by `new`) purely so construction can't fail: `Session::new` has no
/// `Result`, which keeps callers simple, and the one real sed invocation
/// this does is cheap enough overall that there's no reason to eagerly run
/// it before the caller is ready to look at the result.
pub struct Session {
    lines: Vec<String>,
    wrapped_script: String,
    hold_active: bool,
    blocks: Vec<Block>,
    computed: bool,
}

impl Session {
    pub fn new(lines: Vec<String>, user_script: &str, hold_active: bool) -> Self {
        let wrapped_script = runner::build_wrapped_script(user_script);
        Session {
            lines,
            wrapped_script,
            hold_active,
            blocks: Vec::new(),
            computed: false,
        }
    }

    fn ensure_computed(&mut self) -> Result<()> {
        if self.computed {
            return Ok(());
        }
        let cycles = runner::run_full(&self.wrapped_script, &self.lines)?;

        let mut blocks = Vec::with_capacity(cycles.len());
        // 1-indexed count of real input lines consumed so far — 0 means
        // none yet, which conveniently is also block 0's 0-indexed start.
        let mut prev_end = 0usize;
        for cycle in cycles {
            anyhow::ensure!(
                cycle.end_line > prev_end,
                "sed's reported line number didn't advance ({} -> {}) — malformed instrumentation",
                prev_end,
                cycle.end_line
            );
            blocks.push(Block {
                start: prev_end, // 0-indexed start == previous 1-indexed end
                end: cycle.end_line - 1,
                pattern_after: cycle.pattern_space,
                printed: cycle.printed,
                hold_after: self.hold_active.then_some(cycle.hold_space),
            });
            prev_end = cycle.end_line;
        }
        anyhow::ensure!(
            prev_end == self.lines.len(),
            "script only consumed {} of {} input lines — early exit (q/Q) isn't supported yet",
            prev_end,
            self.lines.len()
        );

        self.blocks = blocks;
        self.computed = true;
        Ok(())
    }

    /// 0-indexed. Triggers the one-time full run on first call.
    pub fn get(&mut self, idx: usize) -> Result<&Block> {
        self.ensure_computed()?;
        Ok(&self.blocks[idx])
    }

    pub fn block_count(&mut self) -> Result<usize> {
        self.ensure_computed()?;
        Ok(self.blocks.len())
    }

    pub fn hold_active(&self) -> bool {
        self.hold_active
    }

    /// Defaults to `true` (i.e. "don't suppress output") if `idx` is ever
    /// queried before `ensure_computed` has run — this can't happen through
    /// `ui`/`main`'s normal call order (they always `get` a block before
    /// asking about it), so this is a defensive fallback, not a real code
    /// path; picking the fail-safe direction (show it) rather than
    /// silently dropping content if that invariant is ever violated.
    pub fn printed(&self, idx: usize) -> bool {
        self.blocks.get(idx).map(|b| b.printed).unwrap_or(true)
    }

    /// `None` only in the same "queried before computed" situation as
    /// `printed` above — real callers always check `printed`/call `get`
    /// first, so `main.rs` unwrapping this is safe in practice, not
    /// reflecting an expected-to-happen case.
    pub fn cached_pattern(&self, idx: usize) -> Option<&str> {
        self.blocks.get(idx).map(|b| b.pattern_after.as_str())
    }

    /// The raw input line(s) making up this block, joined by real newlines.
    pub fn raw_input(&self, idx: usize) -> String {
        self.blocks[idx].raw_input(&self.lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(str::to_string).collect()
    }

    /// End-to-end check of the block grouping + "accept all" output
    /// reconstruction main.rs relies on, for a script that merges lines via
    /// `N` (bypassing the TUI entirely).
    #[test]
    fn swap_adjacent_lines_blocks_and_accepts_correctly() {
        let script = r"$!N
s/(.*)\n(.*)/\2\n\1/";
        let mut session = Session::new(lines("a\nb\nc\nd\n"), script, false);

        assert_eq!(session.block_count().unwrap(), 2);

        let b0 = session.get(0).unwrap();
        assert_eq!((b0.start, b0.end), (0, 1));
        assert_eq!(b0.pattern_after, "b\na");
        assert!(b0.printed);

        let b1 = session.get(1).unwrap();
        assert_eq!((b1.start, b1.end), (2, 3));
        assert_eq!(b1.pattern_after, "d\nc");

        // simulate "accept everything" the way main.rs reconstructs output
        let mut output = String::new();
        for i in 0..session.block_count().unwrap() {
            if !session.printed(i) {
                continue;
            }
            output.push_str(session.cached_pattern(i).unwrap());
            output.push('\n');
        }
        assert_eq!(output, "b\na\nd\nc\n");
    }
}
