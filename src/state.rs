pub struct LineRecord {
    pub input: String,
    pub pattern_after: String,
    /// Whether this line's cycle actually reaches a print (autoprint or
    /// explicit `p`) in the real script — `false` means `d` deleted it and
    /// it contributes no output at all, no matter the user's decision.
    pub printed: bool,
    pub hold_after: Option<String>,
}
