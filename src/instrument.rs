use regex::Regex;

const TAG_PATTERN: &str = "\x01P\x01";
const TAG_DELETED: &str = "\x01X\x01";
const TAG_HOLD: &str = "\x01H\x01";
// Embedded real newlines (from N/H/G on multiline content) would otherwise
// break our one-line-per-tag parsing, so we swap them for this placeholder
// before printing and restore it in `parse_cycles`.
const NL_PLACEHOLDER: char = '\x02';

const DEL_LABEL: &str = "ISED_DEL";
const END_LABEL: &str = "ISED_END";
const TOP_LABEL: &str = "ISED_TOP";

/// A bare `d` normally deletes pattern space and jumps straight to the next
/// cycle, skipping any script text after it — including our instrumentation.
/// We rewrite it to branch into our own "deleted" tag block instead, so we
/// still learn the pattern/hold space at the moment of deletion and record
/// that this cycle produced no real output. `d` takes no argument, so this
/// looks for the letter standing alone as a command token.
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
pub fn wrap_script(user_script: &str) -> String {
    let rewritten = rewrite_d_upper(&rewrite_delete(user_script));
    format!(
        ":{top}\n\
         {{\n{script}\n}}\n\
         s/\\n/\\x02/g\n\
         s/^/{ptag}/\n\
         p\n\
         b {end}\n\
         :{del}\n\
         s/\\n/\\x02/g\n\
         s/^/{dtag}/\n\
         p\n\
         :{end}\n\
         =\n\
         x\n\
         s/\\n/\\x02/g\n\
         s/^/{htag}/\n\
         p\n\
         s/^{htag}//\n\
         s/\\x02/\\n/g\n\
         x\n",
        script = rewritten,
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
    /// Whether this cycle would really print (autoprint reached) vs `d`/`D`
    /// deleting the pattern space and suppressing output entirely.
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
    }

    cycles
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::process::{Command, Stdio};

    fn run_wrapped(script: &str, input: &str) -> Vec<Cycle> {
        let wrapped = wrap_script(script);
        let mut child = Command::new("sed")
            .arg("-n")
            .arg(&wrapped)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        parse_cycles(&String::from_utf8_lossy(&out.stdout))
    }

    fn real_sed_output(script: &str, input: &str) -> String {
        let mut child = Command::new("sed")
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
        let out = child.wait_with_output().unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
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
        // classic "swap adjacent lines" idiom
        let script = "$!N\ns/\\(.*\\)\\n\\(.*\\)/\\2\\n\\1/";
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
