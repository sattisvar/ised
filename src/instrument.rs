//! Turns a user-supplied sed script into a self-instrumented version of
//! itself: same script, same real behaviour, but it also reports its own
//! internal state (pattern space, hold space, current line number) as it
//! runs, by appending extra sed commands after the user's script and
//! rewriting the couple of commands (`d`, `D`) that would otherwise skip
//! past that appended code.
//!
//! Why instrument sed instead of reimplementing it: the sed engine's actual
//! behaviour (regex quirks, GNU extensions, `N`-at-EOF handling, etc.) is
//! exactly what we want to preview, so the only way to guarantee we show
//! the truth is to have the same engine (`sed_rs`) run the real script and
//! ask it to also tell us what it's doing, rather than modeling sed
//! ourselves and risking divergence.
//!
//! Note `sed_rs` is extended-regex-only (no BRE mode) and does not
//! interpret `\xHH` escapes in scripts — which is why the placeholder
//! below is embedded as a literal control character.
//!
//! The output is designed to be parsed strictly line-by-line by
//! `parse_cycles`, which is why every value we extract (pattern space, hold
//! space) gets its embedded real newlines swapped for `NL_PLACEHOLDER`
//! before being tagged and printed, then swapped back on the way out.

use regex::Regex;

// \x01/\x02 (SOH/STX) rather than something printable: any printable tag
// could collide with real line content the user is editing; these bytes
// essentially never occur in text files sed is used on. It's a heuristic,
// not a guarantee — see the module-level limitations note.
const TAG_PATTERN: &str = "\x01P\x01";
const TAG_DELETED: &str = "\x01X\x01";
const TAG_HOLD: &str = "\x01H\x01";
// Embedded real newlines (from N/H/G on multiline content) would otherwise
// break our one-line-per-tag parsing, so we swap them for this placeholder
// before printing and restore it in `parse_cycles`.
const NL_PLACEHOLDER: char = '\x02';

// Label names sed branches to. Must not collide with any label the user's
// own script defines — "ISED_*" is chosen to be unlikely, not guaranteed;
// a script that happens to define e.g. `:ISED_END` itself would silently
// misbehave (last-writer-wins on the label, since sed doesn't error on
// duplicate labels). Not currently validated against.
const DEL_LABEL: &str = "ISED_DEL";
const END_LABEL: &str = "ISED_END";
const TOP_LABEL: &str = "ISED_TOP";

/// A bare `d` normally deletes pattern space and jumps straight to the next
/// cycle, skipping any script text after it — including our instrumentation.
/// We rewrite it to branch into our own "deleted" tag block instead, so we
/// still learn the pattern/hold space at the moment of deletion and record
/// that this cycle produced no real output. `d` takes no argument, so this
/// looks for the letter standing alone as a command token.
///
/// The boundary character class in the regex is a heuristic covering the
/// common ways a command can be preceded/followed in real scripts
/// (separators, block braces, digit/`$`/`,`/`!` from addresses, `/` from a
/// regex-address delimiter) — not a real sed grammar parse. It can misfire
/// on unusual constructs (e.g. a custom regex-address delimiter other than
/// `/`, like `\%...%d`) by failing to rewrite a `d` that should have been,
/// silently reverting to the old "skips instrumentation" behaviour for that
/// command rather than erroring. Same caveat applies to `rewrite_d_upper`
/// and `uses_hold_space` below, which use the same boundary class.
fn rewrite_delete(user_script: &str) -> String {
    let re = Regex::new(r"(^|[;{}\n\s0-9$,!/])d($|[;}\n\s])").unwrap();
    re.replace_all(user_script, |caps: &regex::Captures| {
        format!("{}b {}{}", &caps[1], DEL_LABEL, &caps[2])
    })
    .into_owned()
}

/// A bare `D`: if pattern space has no embedded newline, behaves exactly
/// like `d` (branch to our deleted-tag path). Otherwise it deletes up to
/// and including the first embedded newline and restarts the script *from
/// the top, without reading new input* — the classic sliding-window loop.
/// We reproduce that restart-without-read behaviour with our own `t`-gated
/// branch back to a label placed before the user's script.
///
/// The replacement is a `{ ... }` group rather than a single command. This
/// matters because sed addresses (the possibly-nonempty text captured in
/// group 1, e.g. `$` in `$D`) apply to exactly one command *or* one `{}`
/// block — since that address text is left untouched right before our
/// group, `$D` naturally becomes `${ ... }`, still correctly scoped,
/// without this function ever needing to parse what the address actually
/// was.
fn rewrite_d_upper(user_script: &str) -> String {
    let re = Regex::new(r"(^|[;{}\n\s0-9$,!/])D($|[;}\n\s])").unwrap();
    re.replace_all(user_script, |caps: &regex::Captures| {
        format!(
            "{}{{ s/^[^\\n]*\\n//; t {top}; b {del} }}{}",
            &caps[1],
            &caps[2],
            top = TOP_LABEL,
            del = DEL_LABEL,
        )
    })
    .into_owned()
}

/// Wraps a user sed script so that, at the end of every *external* cycle
/// (one that really reads/would-read from the input stream — `D`'s
/// restart-without-read loop is invisible to this), it emits: the final
/// pattern space (tagged `printed` if a real autoprint would have happened,
/// `deleted` if `d`/`D` fired), the real current line number (via `=`, so
/// callers can tell how many raw input lines — possibly more than one, via
/// `N` — this cycle actually consumed), and the hold space. Each tag is on
/// its own (single, newline-free) line. Hold space is restored exactly
/// afterwards so the rest of the script's semantics are unaffected on the
/// next cycle; pattern space is left mutated since it is discarded (by a
/// real next-line read) before it would matter.
/// This assumes the caller invokes sed with `-n` (see `runner::run_full`) —
/// the `p` right after the user's script stands in for autoprint, which we
/// disable globally so we control exactly when it fires (never, on the
/// `d`/`D` path). Running this wrapped script without `-n` would double
/// every real print.
///
/// Structure of the generated script, in order:
///  1. `:TOP` — sits *before* the user's script so `D`'s rewritten branch
///     (see `rewrite_d_upper`) can loop back into it, re-entering the
///     script body with the trimmed pattern space and no new read.
///  2. the user's script (with `d`/`D` rewritten).
///  3. the "printed" path — reached only if the script fell through
///     normally (no `d`/`D` fired) — tags and prints pattern space, then
///     branches past the deleted path below.
///  4. `:DEL`, the "deleted" path — reached only via a rewritten `d`/`D` —
///     tags and prints pattern space the same way, just under a different
///     tag, then falls through to the same convergence point as step 3.
///  5. `:END` — the one point reached exactly once per *external* cycle,
///     whichever path got here — `=` reports the real current line number
///     and the hold space gets tagged, printed, and (unlike pattern space
///     above, which is left mutated since it's discarded by the next real
///     read) restored exactly, or every later h/H/g/G/x in the user's
///     script would silently operate on corrupted hold-space content.
pub fn wrap_script(user_script: &str) -> String {
    let rewritten = rewrite_d_upper(&rewrite_delete(user_script));
    format!(
        ":{top}\n\
         {{\n{script}\n}}\n\
         s/\\n/{nl}/g\n\
         s/^/{ptag}/\n\
         p\n\
         b {end}\n\
         :{del}\n\
         s/\\n/{nl}/g\n\
         s/^/{dtag}/\n\
         p\n\
         :{end}\n\
         =\n\
         x\n\
         s/\\n/{nl}/g\n\
         s/^/{htag}/\n\
         p\n\
         s/^{htag}//\n\
         s/{nl}/\\n/g\n\
         x\n",
        script = rewritten,
        // Written as the literal control character, not a \x02 escape:
        // sed_rs does not interpret \xHH escapes in scripts (it would take
        // them literally), but literal control bytes in the script work.
        nl = NL_PLACEHOLDER,
        ptag = TAG_PATTERN,
        dtag = TAG_DELETED,
        htag = TAG_HOLD,
        top = TOP_LABEL,
        del = DEL_LABEL,
        end = END_LABEL,
    )
}

/// Heuristic: does this script ever touch the hold space (h/H/g/G/x)?
/// Sed hold commands take no argument, so we look for one of those letters
/// standing alone as a command token (bounded by ; { } newline or whitespace).
pub fn uses_hold_space(user_script: &str) -> bool {
    let re = Regex::new(r"(^|[;{}\n\s0-9$,!/])[hHgGx]($|[;}\n\s])").unwrap();
    re.is_match(user_script)
}

pub struct Cycle {
    pub pattern_space: String,
    /// Whether this cycle reaches *our* print (i.e. autoprint would have
    /// fired in the real script) vs `d`/`D` deleting the pattern space and
    /// routing to the deleted path instead.
    ///
    /// This is *not* "whether the real script produced any output at all"
    /// — a script with its own explicit `p`/`P`/`w` mid-script (the classic
    /// `N;P;D` "print sliding window" idiom, for instance) can produce real
    /// output on the deleted path too, via that explicit print, which this
    /// tool doesn't currently track or correlate to a block (see the note
    /// at the bottom of `parse_cycles`'s loop). For such scripts, `printed
    /// == false` blocks may still have contributed real output that this
    /// tool won't show or let the user accept/reject.
    pub printed: bool,
    pub hold_space: String,
    /// The real 1-indexed line number reached when this cycle ended — i.e.
    /// the last raw input line this cycle consumed. Consecutive cycles'
    /// `end_line` values mark off which (possibly multi-line, via `N`)
    /// block of the input each one covers.
    pub end_line: usize,
}

/// Parses the tagged stdout produced by a script wrapped with `wrap_script`,
/// returning one `Cycle` per *external* cycle (see `wrap_script`), in order.
pub fn parse_cycles(stdout: &str) -> Vec<Cycle> {
    let mut cycles = Vec::new();
    let mut pending_pattern: Option<(String, bool)> = None;
    let mut pending_nr: Option<usize> = None;

    for line in stdout.split('\n') {
        if let Some(rest) = line.strip_prefix(TAG_PATTERN) {
            pending_pattern = Some((rest.replace(NL_PLACEHOLDER, "\n"), true));
        } else if let Some(rest) = line.strip_prefix(TAG_DELETED) {
            pending_pattern = Some((rest.replace(NL_PLACEHOLDER, "\n"), false));
        } else if let Some(rest) = line.strip_prefix(TAG_HOLD) {
            let (pattern_space, printed) = pending_pattern.take().unwrap_or_default();
            let Some(end_line) = pending_nr.take() else {
                continue; // malformed/unexpected output; skip rather than panic
            };
            cycles.push(Cycle {
                pattern_space,
                printed,
                hold_space: rest.replace(NL_PLACEHOLDER, "\n"),
                end_line,
            });
        } else if let Ok(nr) = line.parse::<usize>() {
            pending_nr = Some(nr);
        }
        // any other line is real program output (only relevant if the
        // script itself prints extra things via `p`/`P`) — ignored for now.
        //
        // Note this means any explicit p/P/w output in the user's own
        // script that happens to be a bare integer would be silently
        // (mis)read as our `=` line number here, and non-numeric explicit
        // output is silently dropped. Both are instances of the same
        // unaddressed gap: explicit output isn't correlated to a block at
        // all (see the `printed` field's doc comment for the broader
        // implication this has for scripts like `N;P;D`).
    }

    cycles
}

#[cfg(test)]
mod tests {
    use super::*;
    use sed_rs::Sed;

    fn run_wrapped(script: &str, input: &str) -> Vec<Cycle> {
        let wrapped = wrap_script(script);
        let stdout = Sed::new(&wrapped).unwrap().quiet(true).eval(input).unwrap();
        parse_cycles(&stdout)
    }

    fn real_sed_output(script: &str, input: &str) -> String {
        sed_rs::eval(script, input).unwrap()
    }

    #[test]
    fn hold_space_detection() {
        assert!(uses_hold_space("H;s/./&&/"));
        assert!(uses_hold_space("1h;2,$G"));
        assert!(!uses_hold_space("s/hello/world/"));
        assert!(!uses_hold_space("s/gone/here/")); // 'g'/'h' inside words, not commands
    }

    #[test]
    fn wrap_and_parse_roundtrip_multiline_hold() {
        let cycles = run_wrapped("H;s/./&&/", "a\nb\nc\n");
        assert_eq!(cycles.len(), 3);
        // H appends pattern space to hold *before* the s/// in this script runs.
        assert_eq!(cycles[0].pattern_space, "aa");
        assert!(cycles[0].printed);
        assert_eq!(cycles[0].end_line, 1);
        assert_eq!(cycles[0].hold_space, "\na");
        assert_eq!(cycles[1].pattern_space, "bb");
        assert_eq!(cycles[1].end_line, 2);
        assert_eq!(cycles[1].hold_space, "\na\nb");
        assert_eq!(cycles[2].pattern_space, "cc");
        assert_eq!(cycles[2].end_line, 3);
        assert_eq!(cycles[2].hold_space, "\na\nb\nc");
    }

    #[test]
    fn delete_suppresses_output_but_hold_still_tracked() {
        // classic "join lines with a comma" idiom
        let script = "1h\n1!H\n$!d\n$ {\n    x\n    s/\\n/, /g\n}\n";
        let cycles = run_wrapped(script, "apple\nbanana\ncherry\n");
        assert_eq!(cycles.len(), 3);

        assert!(!cycles[0].printed);
        assert_eq!(cycles[0].end_line, 1);
        assert_eq!(cycles[0].hold_space, "apple");

        assert!(!cycles[1].printed);
        assert_eq!(cycles[1].end_line, 2);
        assert_eq!(cycles[1].hold_space, "apple\nbanana");

        assert!(cycles[2].printed);
        assert_eq!(cycles[2].end_line, 3);
        assert_eq!(cycles[2].pattern_space, "apple, banana, cherry");

        assert_eq!(
            real_sed_output(script, "apple\nbanana\ncherry\n"),
            "apple, banana, cherry\n"
        );
    }

    #[test]
    fn n_merges_pairs_of_lines_into_one_cycle() {
        // classic "swap adjacent lines" idiom (ERE groups — sed_rs is
        // extended-regex-only, there is no BRE mode)
        let script = "$!N\ns/(.*)\\n(.*)/\\2\\n\\1/";
        let cycles = run_wrapped(script, "a\nb\nc\nd\n");

        // lines 1-2 merge into one cycle (N), likewise 3-4.
        assert_eq!(cycles.len(), 2);
        assert_eq!(cycles[0].end_line, 2);
        assert_eq!(cycles[0].pattern_space, "b\na");
        assert_eq!(cycles[1].end_line, 4);
        assert_eq!(cycles[1].pattern_space, "d\nc");

        assert_eq!(real_sed_output(script, "a\nb\nc\nd\n"), "b\na\nd\nc\n");
    }

    #[test]
    fn d_upper_loops_without_reading_new_line() {
        // `$!N;D` (N guarded so it never hits real EOF) slides a one-line
        // window across the whole file without ever printing anything —
        // every D restart loops back to the top *without* an external
        // read, so all three lines end up folded into a single cycle.
        let script = "$!N\nD";
        let cycles = run_wrapped(script, "a\nb\nc\n");

        assert_eq!(cycles.len(), 1);
        assert!(!cycles[0].printed);
        assert_eq!(cycles[0].end_line, 3);

        assert_eq!(real_sed_output(script, "a\nb\nc\n"), "");
    }

    #[test]
    fn n_at_eof_under_forced_dash_n_is_a_known_gap() {
        // Plain `N;D` (unguarded) relies on GNU sed's default behaviour of
        // autoprinting-then-exiting when `N` hits end of input with no next
        // line — but that fallback is itself gated on autoprint being on,
        // and our wrapper always runs with `-n`. So the very last cycle's
        // tag never fires here, and this tool would show nothing for the
        // final line even though real (unwrapped) sed prints it — a known,
        // documented limitation of the current instrumentation.
        let script = "N\nD";
        let cycles = run_wrapped(script, "a\nb\nc\n");
        assert!(cycles.is_empty());

        assert_eq!(real_sed_output(script, "a\nb\nc\n"), "c\n");
    }
}
