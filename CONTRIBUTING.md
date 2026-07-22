# Contributing to ised

Thanks for considering a contribution.

## License

ised is licensed under the GNU General Public License v3.0 or later
(GPLv3+), same as GNU sed itself — see [LICENSE.md](LICENSE.md). By
submitting a contribution, you agree it's licensed under the same terms.

## Getting started

```
git clone <repo>
cd ised
cargo build
cargo test
```

Requires a working Rust toolchain and GNU sed on `PATH` (used at runtime,
and by the test suite in `src/instrument.rs`, which shells out to real
`sed` to check the instrumented script against actual sed behavior).

## Before you open a PR

- `cargo test` passes.
- `cargo fmt` applied.
- `cargo clippy` has no new warnings.
- For changes to `src/instrument.rs` (the script-rewriting/tagging logic),
  add a test that runs both the wrapped script and the real, unwrapped
  script (see `run_wrapped` / `real_sed_output` helpers already in that
  file) and asserts they agree — this is what catches instrumentation bugs
  that would silently corrupt sed semantics.

## Reporting bugs

Open an issue with:
- the sed script and input you ran
- what `ised` showed vs. what real `sed` produces for the same script/input
- your `sed --version` (GNU sed only is supported; BSD/macOS sed is not)

## Scope

Known gaps are tracked in the README's "Known limitations" section (e.g.
`q`/`Q` early exit isn't supported yet). PRs closing those are welcome —
open an issue first for anything larger than a small fix, so the approach
can be discussed before you sink time into it.

## Code style

Keep additions minimal and consistent with the existing style: no comments
except where they explain a non-obvious *why* (see `src/instrument.rs` for
the pattern), small focused functions, tests colocated in `#[cfg(test)]`
modules next to the code they cover.
