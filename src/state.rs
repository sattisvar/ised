/// One reviewable unit: a run of one or more consecutive raw input lines
/// that a single external sed cycle consumed together (more than one only
/// when the script uses `N` to read ahead).
pub struct Block {
    /// 0-indexed, inclusive range of raw input lines this block covers.
    pub start: usize,
    pub end: usize,
    pub pattern_after: String,
    /// Whether this cycle actually reaches a print (autoprint or explicit
    /// `p`) in the real script — `false` means `d`/`D` deleted it and it
    /// contributes no output at all, no matter the user's decision.
    pub printed: bool,
    pub hold_after: Option<String>,
}

impl Block {
    pub fn raw_input(&self, lines: &[String]) -> String {
        lines[self.start..=self.end].join("\n")
    }
}
