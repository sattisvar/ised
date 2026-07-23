# ised — interactive sed

`ised` runs a `sed` script against a file (or stdin) and lets you walk
through the result *cycle by cycle* in a terminal UI, seeing a word-level
diff of pattern space before/after, before deciding whether to keep each
change. It's `sed` with a review step, useful when a script's effect on
tricky lines (multi-line `N` blocks, `d`/`D` deletes, hold-space tricks)
isn't obvious just from reading the script.

## How it works

1. **Instrument** (`src/instrument.rs`): the user's sed script is rewritten
   so that at the end of every external cycle it emits three tagged lines —
   the resulting pattern space, whether it would really print (vs. get
   deleted by `d`/`D`), and the hold space — plus the real input line number
   via `=`. `d`/`D` are rewritten to branch into this instrumentation
   instead of skipping it outright, and `D`'s restart-without-read loop is
   reproduced faithfully.
2. **Run once** (`src/runner.rs`): the instrumented script is run through
   the [`sed_rs`](https://docs.rs/sed-rs) engine in quiet (`-n`) mode over
   the *entire* input in a single invocation — required to keep `$`-anchored
   addressing correct.
3. **Group into blocks** (`src/session.rs`, `src/state.rs`): the tagged
   output is parsed back into cycles and grouped into `Block`s — one or more
   consecutive raw input lines a single cycle consumed together (more than
   one only when the script uses `N`).
4. **Review** (`src/ui.rs`): a `ratatui` TUI lists all blocks, auto-skipping
   no-op ones, and shows a word-diff (via the `similar` crate) of each
   changed block's pattern space, plus the hold space if the script touches
   it. You accept or reject each real change.
5. **Reconstruct output** (`src/main.rs`): accepted blocks emit the
   transformed pattern space; rejected or undecided blocks emit the raw
   input untouched; blocks that `d`/`D` deleted emit nothing regardless of
   your decision (there's nothing to keep).

If the script is a no-op on the whole input, the TUI is skipped entirely and
the input is passed through unchanged.

## Usage

```
ised '<sed-script>' [file] [-o|--output <file>]
```

- `script` — a sed script, e.g. `'s/foo/bar/'`.
- `file` — input file; omit to read stdin.
- `-o, --output <file>` — write result here instead of stdout.

### TUI keys

| Key       | Action                          |
|-----------|----------------------------------|
| `y`       | accept this block's change       |
| `n`       | reject (keep raw input)          |
| `p` / `↑` | go back one block                |
| `g`       | jump to first block              |
| `q` / Esc | finish — write out decisions so far |

### Example

```
$ printf 'apple\nbanana\ncherry\n' | ised 's/^a/A/'
```

Opens the TUI on line 1 (`apple` → `Apple`, `banana` unaffected — the tool
skips it automatically since it's a no-op — `cherry` → `cherry` unaffected).
Press `y` to keep `Apple`, and since the rest have no diff, it exits
immediately with:

```
Apple
banana
cherry
```

Swap-adjacent-lines example (uses `N`, so blocks span two raw lines):

```
$ printf 'a\nb\nc\nd\n' | ised '$!N
s/\(.*\)\n\(.*\)/\2\n\1/'
```

Each reviewable block here covers 2 input lines (`1-2`, `3-4`) since `N`
pulls a second line into pattern space before the substitution runs.

## Dependencies

Rust crate dependencies (`Cargo.toml`):

- [`ratatui`](https://crates.io/crates/ratatui) `0.29` — terminal UI framework
- [`crossterm`](https://crates.io/crates/crossterm) `0.28` — terminal backend for ratatui (raw mode, alternate screen, key events)
- [`similar`](https://crates.io/crates/similar) `2.6` — word-level text diffing
- [`clap`](https://crates.io/crates/clap) `4` (derive feature) — CLI argument parsing
- [`anyhow`](https://crates.io/crates/anyhow) `1` — error handling
- [`regex`](https://crates.io/crates/regex) `1` — rewriting `d`/`D` commands in the user's script
- [`sed-rs`](https://crates.io/crates/sed-rs) `1.1` — GNU-compatible sed
  engine that executes the (instrumented) script in-process; no external
  `sed` binary is needed.

## Known limitations

- Regular expressions are always **extended** (ERE), as with `sed -E`:
  `sed_rs` has no BRE mode, so write groups as `(...)`, not `\(...\)`.
  Scripts written for GNU sed's default BRE syntax need adjusting.
- `\xHH`/`\oNNN` escapes in scripts are not interpreted by `sed_rs` — they
  are taken literally.
- `q`/`Q` (early exit) aren't supported yet — the tool expects the script to
  consume the whole input.
- A trailing `N` at true end-of-input under `-n` (e.g. plain `N;D` on the
  last line) is a known gap in the instrumentation — see the test
  `n_at_eof_under_forced_dash_n_is_a_known_gap` in `src/instrument.rs` for
  details.

## Building

```
cargo build --release
```

## Testing

```
cargo test
```
