use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block as UiBlock, Borders, List, ListItem, Paragraph};
use ratatui::{Frame, Terminal};
use similar::{ChangeTag, TextDiff};
use std::io::stdout;

use crate::session::Session;

/// One decision per block: `Some(true)` = accepted (emit the transformed
/// text), `Some(false)` = rejected (emit the raw lines), `None` = never
/// reviewed (emit the raw lines — undecided defaults to "don't touch it").
///
/// Returns `Ok(None)` if the whole script is a no-op — nothing to review,
/// no TUI is shown at all.
pub fn run(session: &mut Session) -> Result<Option<Vec<Option<bool>>>> {
    let total = session.block_count()?;
    let mut decisions: Vec<Option<bool>> = vec![None; total];
    let Some(mut cursor) = find_first_diff(session, &mut decisions)? else {
        return Ok(None);
    };

    enable_raw_mode()?;
    stdout().execute(EnterAlternateScreen)?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout());
    let mut terminal = Terminal::new(backend)?;

    let result = loop {
        terminal.draw(|f| draw(f, session, cursor, &decisions))?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break Ok(()),
                KeyCode::Char('y') => {
                    decisions[cursor] = Some(true);
                    if !advance_past_noops(session, &mut cursor, &mut decisions)? {
                        break Ok(()); // was already on the last block — done
                    }
                }
                KeyCode::Char('n') => {
                    decisions[cursor] = Some(false);
                    if !advance_past_noops(session, &mut cursor, &mut decisions)? {
                        break Ok(());
                    }
                }
                KeyCode::Char('p') | KeyCode::Up => {
                    cursor = cursor.saturating_sub(1);
                }
                KeyCode::Char('g') => cursor = 0,
                _ => {}
            }
        }
    };

    disable_raw_mode()?;
    stdout().execute(LeaveAlternateScreen)?;
    result.map(|()| Some(decisions))
}

/// Scans forward from block 0 for the first one where anything actually
/// changes (pattern space touched, hold space diverges from its initial
/// empty state, or the block gets deleted and produces no real output),
/// marking every no-op block along the way as auto-accepted. Returns `None`
/// if nothing in the whole file changes anything.
fn find_first_diff(session: &mut Session, decisions: &mut [Option<bool>]) -> Result<Option<usize>> {
    let mut prev_hold = session.hold_active().then(String::new);
    for i in 0..decisions.len() {
        let raw = session.raw_input(i);
        let record = session.get(i)?;
        let pattern_changed = raw != record.pattern_after;
        let hold_changed = record.hold_after != prev_hold;
        if pattern_changed || hold_changed || !record.printed {
            return Ok(Some(i));
        }
        decisions[i] = Some(true);
        prev_hold = record.hold_after.clone();
    }
    Ok(None)
}

/// Moves forward from the current (just-decided) block, auto-accepting and
/// skipping any no-op ones (pattern space untouched and hold space
/// unchanged), until it lands on the next one with a real diff. Returns
/// `false` if there was nowhere to advance to (already on the last block).
fn advance_past_noops(
    session: &mut Session,
    cursor: &mut usize,
    decisions: &mut [Option<bool>],
) -> Result<bool> {
    if *cursor + 1 >= decisions.len() {
        return Ok(false);
    }
    loop {
        let prev_hold = session.get(*cursor)?.hold_after.clone();
        *cursor += 1;
        let raw = session.raw_input(*cursor);
        let record = session.get(*cursor)?;
        let pattern_changed = raw != record.pattern_after;
        let hold_changed = record.hold_after != prev_hold;
        if pattern_changed || hold_changed || !record.printed {
            return Ok(true);
        }
        // No-op block: nothing to decide, output would be identical either way.
        decisions[*cursor] = Some(true);
        if *cursor + 1 >= decisions.len() {
            return Ok(false);
        }
    }
}

fn draw(f: &mut Frame, session: &mut Session, cursor: usize, decisions: &[Option<bool>]) {
    let outer = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(f.area());

    let total = decisions.len();
    // Borders take the top and bottom row of the panel.
    let visible = outer[0].height.saturating_sub(2).max(1) as usize;
    let top = if total <= visible {
        0
    } else {
        cursor.saturating_sub(visible / 2).min(total - visible)
    };
    let items: Vec<ListItem> = (top..(top + visible).min(total))
        .map(|i| {
            let marker = match decisions[i] {
                Some(true) => "y",
                Some(false) => "n",
                None => " ",
            };
            // Compact single-row preview: multi-line blocks (via N) show
            // their lines joined with a visible break marker.
            let preview = session.raw_input(i).replace('\n', " ⏎ ");
            let block = session.get(i).expect("already computed by find_first_diff");
            let label = if block.start == block.end {
                format!("{}", block.start + 1)
            } else {
                format!("{}-{}", block.start + 1, block.end + 1)
            };
            let text = format!("{} {:>7}  {}", marker, label, preview);
            let style = if i == cursor {
                Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                match decisions[i] {
                    Some(true) => Style::default().fg(Color::Green),
                    Some(false) => Style::default().fg(Color::Red),
                    None => Style::default(),
                }
            };
            ListItem::new(Line::from(Span::styled(text, style)))
        })
        .collect();
    f.render_widget(
        List::new(items).block(
            UiBlock::default()
                .borders(Borders::ALL)
                .title("input  (y: accept, n: reject, p/up: back, q: quit)"),
        ),
        outer[0],
    );

    let hold_active = session.hold_active();
    let right_constraints = if hold_active {
        vec![Constraint::Percentage(60), Constraint::Percentage(40)]
    } else {
        vec![Constraint::Percentage(100)]
    };
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints(right_constraints)
        .split(outer[1]);

    let raw = session.raw_input(cursor);
    // cursor is always < block_count() by construction (get() is called
    // before cursor is allowed to move there), so this can't fail.
    let record = session.get(cursor).expect("current block already computed");

    let pattern_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(right[0]);
    let (before, after) = diff_paragraphs(&raw, &record.pattern_after, record.printed);
    f.render_widget(before, pattern_rows[0]);
    f.render_widget(after, pattern_rows[1]);

    if hold_active {
        let hold_text = record.hold_after.as_deref().unwrap_or("");
        f.render_widget(
            Paragraph::new(hold_text.to_string())
                .block(UiBlock::default().borders(Borders::ALL).title("hold space")),
            right[1],
        );
    }
}

fn diff_paragraphs<'a>(before: &'a str, after: &'a str, printed: bool) -> (Paragraph<'a>, Paragraph<'a>) {
    let diff = TextDiff::from_words(before, after);

    let mut before_spans = Vec::new();
    let mut after_spans = Vec::new();
    for change in diff.iter_all_changes() {
        match change.tag() {
            ChangeTag::Delete => before_spans.push(Span::styled(
                change.to_string(),
                Style::default().fg(Color::Red).add_modifier(Modifier::CROSSED_OUT),
            )),
            ChangeTag::Insert => after_spans.push(Span::styled(
                change.to_string(),
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            )),
            ChangeTag::Equal => {
                before_spans.push(Span::raw(change.to_string()));
                after_spans.push(Span::raw(change.to_string()));
            }
        }
    }

    let after_title = if printed {
        "pattern space: after (printed)"
    } else {
        "pattern space: after (deleted — no output for this block)"
    };
    let before_p = Paragraph::new(spans_to_lines(before_spans))
        .block(UiBlock::default().borders(Borders::ALL).title("pattern space: before"))
        .wrap(ratatui::widgets::Wrap { trim: false });
    let after_p = Paragraph::new(spans_to_lines(after_spans))
        .block(UiBlock::default().borders(Borders::ALL).title(after_title))
        .wrap(ratatui::widgets::Wrap { trim: false });

    (before_p, after_p)
}

/// Splits a flat run of spans into multiple `Line`s at any literal newline
/// inside a span's text — needed once blocks can span several raw lines
/// (via `N`), since a `Line` widget otherwise renders on a single row.
fn spans_to_lines(spans: Vec<Span<'_>>) -> Text<'_> {
    let mut lines = Vec::new();
    let mut current: Vec<Span> = Vec::new();
    for span in spans {
        let style = span.style;
        let mut parts = span.content.split('\n');
        if let Some(first) = parts.next() {
            current.push(Span::styled(first.to_string(), style));
        }
        for part in parts {
            lines.push(Line::from(std::mem::take(&mut current)));
            current.push(Span::styled(part.to_string(), style));
        }
    }
    lines.push(Line::from(current));
    Text::from(lines)
}
