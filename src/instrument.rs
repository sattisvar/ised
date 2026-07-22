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

/// Wraps a user sed script so that, at the end of every cycle, it emits the
/// final pattern space (tagged `printed` if a real autoprint would have
/// happened, `deleted` if `d` fired) and the hold space, each on their own
/// (single, newline-free) line. Hold space is restored exactly afterwards
/// so the rest of the script's semantics are unaffected on the next cycle;
/// pattern space is left mutated since it is discarded on the next read.
pub fn wrap_script(user_script: &str) -> String {
    let rewritten = rewrite_delete(user_script);
    format!(
        "{{\n{script}\n}}\n\
         s/\\n/\\x02/g\n\
         s/^/{ptag}/\n\
         p\n\
         b {end}\n\
         :{del}\n\
         s/\\n/\\x02/g\n\
         s/^/{dtag}/\n\
         p\n\
         :{end}\n\
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
    /// Whether this cycle would really print (autoprint reached) vs `d`
    /// deleting the pattern space and suppressing output entirely.
    pub printed: bool,
    pub hold_space: String,
}

/// Parses the tagged stdout produced by a script wrapped with `wrap_script`,
/// returning one `Cycle` per input line consumed, in order.
pub fn parse_cycles(stdout: &str) -> Vec<Cycle> {
    let mut cycles = Vec::new();
    let mut pending: Option<(String, bool)> = None;

    for line in stdout.split('\n') {
        if let Some(rest) = line.strip_prefix(TAG_PATTERN) {
            pending = Some((rest.replace(NL_PLACEHOLDER, "\n"), true));
        } else if let Some(rest) = line.strip_prefix(TAG_DELETED) {
            pending = Some((rest.replace(NL_PLACEHOLDER, "\n"), false));
        } else if let Some(rest) = line.strip_prefix(TAG_HOLD) {
            let (pattern_space, printed) = pending.take().unwrap_or_default();
            cycles.push(Cycle {
                pattern_space,
                printed,
                hold_space: rest.replace(NL_PLACEHOLDER, "\n"),
            });
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
        assert_eq!(cycles[0].hold_space, "\na");
        assert_eq!(cycles[1].pattern_space, "bb");
        assert_eq!(cycles[1].hold_space, "\na\nb");
        assert_eq!(cycles[2].pattern_space, "cc");
        assert_eq!(cycles[2].hold_space, "\na\nb\nc");
    }

    #[test]
    fn delete_suppresses_output_but_hold_still_tracked() {
        // classic "join lines with a comma" idiom
        let script = "1h\n1!H\n$!d\n$ {\n    x\n    s/\\n/, /g\n}\n";
        let cycles = run_wrapped(script, "apple\nbanana\ncherry\n");
        assert_eq!(cycles.len(), 3);

        assert!(!cycles[0].printed);
        assert_eq!(cycles[0].hold_space, "apple");

        assert!(!cycles[1].printed);
        assert_eq!(cycles[1].hold_space, "apple\nbanana");

        assert!(cycles[2].printed);
        assert_eq!(cycles[2].pattern_space, "apple, banana, cherry");

        // matches real, unwrapped sed output exactly
        let mut real = Command::new("sed")
            .arg(script)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        real.stdin
            .take()
            .unwrap()
            .write_all(b"apple\nbanana\ncherry\n")
            .unwrap();
        let real_out = real.wait_with_output().unwrap();
        assert_eq!(
            String::from_utf8_lossy(&real_out.stdout),
            "apple, banana, cherry\n"
        );
    }
}
