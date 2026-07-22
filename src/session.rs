use anyhow::Result;

use crate::runner;
use crate::state::LineRecord;

/// Drives sed once over the whole file (a single real invocation — running
/// only a truncated prefix breaks `$`-anchored addressing), then serves
/// each line's real pattern/hold space state from that one run's results.
pub struct Session {
    lines: Vec<String>,
    wrapped_script: String,
    hold_active: bool,
    cache: Vec<Option<LineRecord>>,
    computed: bool,
}

impl Session {
    pub fn new(lines: Vec<String>, user_script: &str, hold_active: bool) -> Self {
        let wrapped_script = runner::build_wrapped_script(user_script);
        let cache = (0..lines.len()).map(|_| None).collect();
        Session {
            lines,
            wrapped_script,
            hold_active,
            cache,
            computed: false,
        }
    }

    /// The transformed pattern space for a line, if it's already been
    /// computed (i.e. the user has reviewed it).
    pub fn cached_pattern(&self, idx: usize) -> Option<&str> {
        self.cache[idx].as_ref().map(|r| r.pattern_after.as_str())
    }

    /// Whether this line's cycle actually produces real output in the real
    /// script (false if `d` deleted it). Defaults to `true` if somehow never
    /// computed, which can't happen once `get` has been called anywhere.
    pub fn printed(&self, idx: usize) -> bool {
        self.cache[idx].as_ref().map(|r| r.printed).unwrap_or(true)
    }

    pub fn total_lines(&self) -> usize {
        self.lines.len()
    }

    pub fn hold_active(&self) -> bool {
        self.hold_active
    }

    fn ensure_computed(&mut self) -> Result<()> {
        if self.computed {
            return Ok(());
        }
        let cycles = runner::run_full(&self.wrapped_script, &self.lines)?;
        anyhow::ensure!(
            cycles.len() == self.lines.len(),
            "script produced {} cycles for {} input lines — commands that change the \
             line count (N, D, n as a mid-script read) aren't supported yet",
            cycles.len(),
            self.lines.len()
        );
        for (i, cycle) in cycles.into_iter().enumerate() {
            self.cache[i] = Some(LineRecord {
                input: self.lines[i].clone(),
                pattern_after: cycle.pattern_space,
                printed: cycle.printed,
                hold_after: self.hold_active.then_some(cycle.hold_space),
            });
        }
        self.computed = true;
        Ok(())
    }

    /// 0-indexed. Triggers the one-time full run on first call.
    pub fn get(&mut self, idx: usize) -> Result<&LineRecord> {
        self.ensure_computed()?;
        Ok(self.cache[idx].as_ref().unwrap())
    }

    /// How many lines are known so far — either 0 (nothing computed yet) or
    /// the whole file (a single run computes everything at once).
    pub fn computed_upto(&self) -> usize {
        if self.computed {
            self.lines.len()
        } else {
            0
        }
    }

    pub fn line_text(&self, idx: usize) -> &str {
        &self.lines[idx]
    }
}
