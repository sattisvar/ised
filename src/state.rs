/// One reviewable unit: a run of one or more consecutive raw input lines
/// that a single external sed cycle consumed together (more than one only
/// when the script uses `N` to read ahead).
pub struct Block {
    /// 0-indexed, inclusive range of raw input lines this block covers.
    pub start: usize,
    pub end: usize,
    pub pattern_after: String,
    /// Mirrors `instrument::Cycle::printed` — see that field's doc comment
    /// for the important caveat around scripts with their own explicit
    /// `p`/`P`. `false` here means `main`'s output reconstruction always
    /// skips this block, regardless of the user's accept/reject decision.
    pub printed: bool,
    /// `None` when the script never touches the hold space at all (decided
    /// once, up front, via `instrument::uses_hold_space`) — that's the
    /// signal `ui` uses to hide the hold-space panel entirely rather than
    /// show an always-empty one. `Some("")` means the script does use hold
    /// space but it happens to be empty at this particular block.
    pub hold_after: Option<String>,
}

impl Block {
    pub fn raw_input(&self, lines: &[String]) -> String {
        lines[self.start..=self.end].join("\n")
    }
}
