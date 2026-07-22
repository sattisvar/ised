mod instrument;
mod runner;
mod session;
mod state;
mod ui;

use anyhow::{Context, Result};
use clap::Parser;
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;

use session::Session;

/// Interactive sed: step line by line, see the diff, watch hold space.
#[derive(Parser)]
struct Args {
    /// sed script, e.g. 's/foo/bar/'
    script: String,

    /// input file (reads stdin if omitted)
    file: Option<PathBuf>,

    /// write the result here instead of stdout
    #[arg(short, long)]
    output: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    let input = match &args.file {
        Some(path) => fs::read_to_string(path)
            .with_context(|| format!("failed to read {}", path.display()))?,
        None => {
            let mut buf = String::new();
            io::stdin().read_to_string(&mut buf)?;
            buf
        }
    };

    let lines: Vec<String> = input.lines().map(str::to_string).collect();
    if lines.is_empty() {
        eprintln!("no input lines");
        return Ok(());
    }

    let hold_active = instrument::uses_hold_space(&args.script);
    let mut session = Session::new(lines, &args.script, hold_active);

    let Some(decisions) = ui::run(&mut session)? else {
        eprintln!("no changes: this script does not modify any line of the input");
        return write_output(&args.output, &input);
    };

    let mut output = String::new();
    for (i, decision) in decisions.iter().enumerate() {
        // A block that `d`/`D` deleted never produces real output, no
        // matter what the user decided about its (informational-only) diff.
        if !session.printed(i) {
            continue;
        }
        let raw = session.raw_input(i);
        let text = match decision {
            Some(true) => session.cached_pattern(i).unwrap_or(&raw).to_string(),
            _ => raw,
        };
        output.push_str(&text);
        output.push('\n');
    }
    write_output(&args.output, &output)
}

fn write_output(path: &Option<PathBuf>, content: &str) -> Result<()> {
    match path {
        Some(path) => fs::write(path, content)
            .with_context(|| format!("failed to write {}", path.display())),
        None => {
            print!("{}", content);
            Ok(())
        }
    }
}
