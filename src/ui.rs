use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::ExecutableCommand;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::{Frame, Terminal};
use similar::{ChangeTag, TextDiff};
use std::io::stdout;

use crate::session::Session;

/// One decision per line: `Some(true)` = accepted (emit the transformed
/// line), `Some(false)` = rejected (emit the raw line), `None` = never
/// reviewed (emit the raw line — undecided defaults to "don't touch it").
///
/// Returns `Ok(None)` if the whole script is a no-op — nothing to review,
/// no TUI is shown at all.
pub fn run(session: &mut Session) -> Result<Option<Vec<Option<bool>>>> {
    let mut decisions: Vec<Option<bool>> = vec![None; session.total_lines()];
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
                        break Ok(()); // was already on the last line — done
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

/// Scans forward from line 0 for the first line where anything actually
/// changes (pattern space touched, hold space diverges from its initial
/// empty state, or the line gets deleted and produces no real output),
/// marking every no-op line along the way as auto-accepted. Returns `None`
/// if no line in the whole file changes anything.
fn find_first_diff(session: &mut Session, decisions: &mut [Option<bool>]) -> Result<Option<usize>> {
    let mut prev_hold = session.hold_active().then(String::new);
    for i in 0..session.total_lines() {
        let record = session.get(i)?;
        let pattern_changed = record.input != record.pattern_after;
        let hold_changed = record.hold_after != prev_hold;
        if pattern_changed || hold_changed || !record.printed {
            return Ok(Some(i));
        }
        decisions[i] = Some(true);
        prev_hold = record.hold_after.clone();
    }
    Ok(None)
}

/// Moves forward from the current (just-decided) line, auto-accepting and
/// skipping any no-op lines (pattern space untouched and hold space
/// unchanged), until it lands on the next line with a real diff. Returns
/// `false` if there was nowhere to advance to (already on the last line).
fn advance_past_noops(
    session: &mut Session,
    cursor: &mut usize,
    decisions: &mut [Option<bool>],
) -> Result<bool> {
    if *cursor + 1 >= session.total_lines() {
        return Ok(false);
    }
    loop {
        let prev_hold = session.get(*cursor)?.hold_after.clone();
        *cursor += 1;
        let record = session.get(*cursor)?;
        let pattern_changed = record.input != record.pattern_after;
        let hold_changed = record.hold_after != prev_hold;
        if pattern_changed || hold_changed || !record.printed {
            return Ok(true);
        }
        // No-op line: nothing to decide, output would be identical either way.
        decisions[*cursor] = Some(true);
        if *cursor + 1 >= session.total_lines() {
            return Ok(false);
        }
    }
}

fn draw(f: &mut Frame, session: &mut Session, cursor: usize, decisions: &[Option<bool>]) {
    let outer = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
        .split(f.area());

    let computed_upto = session.computed_upto();
    let total = session.total_lines();
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
            let text = if i < computed_upto {
                format!("{} {:>4}  {}", marker, i + 1, session.line_text(i))
            } else {
                format!("{} {:>4}  {} (not yet run)", marker, i + 1, session.line_text(i))
            };
            let style = if i == cursor {
                Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                match decisions[i] {
                    Some(true) => Style::default().fg(Color::Green),
                    Some(false) => Style::default().fg(Color::Red),
                    None if i >= computed_upto => Style::default().fg(Color::DarkGray),
                    None => Style::default(),
                }
            };
            ListItem::new(Line::from(Span::styled(text, style)))
        })
        .collect();
    f.render_widget(
        List::new(items).block(
            Block::default()
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

    // cursor is always <= computed_upto - 1 by construction (get() is called
    // before cursor is allowed to move), so this can't fail.
    let record = session.get(cursor).expect("current line already computed");

    let pattern_rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(right[0]);
    let (before, after) = diff_paragraphs(&record.input, &record.pattern_after, record.printed);
    f.render_widget(before, pattern_rows[0]);
    f.render_widget(after, pattern_rows[1]);

    if hold_active {
        let hold_text = record.hold_after.as_deref().unwrap_or("");
        f.render_widget(
            Paragraph::new(hold_text.to_string())
                .block(Block::default().borders(Borders::ALL).title("hold space")),
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
        "pattern space: after (deleted — no output for this line)"
    };
    let before_p = Paragraph::new(Line::from(before_spans))
        .block(Block::default().borders(Borders::ALL).title("pattern space: before"))
        .wrap(ratatui::widgets::Wrap { trim: false });
    let after_p = Paragraph::new(Line::from(after_spans))
        .block(Block::default().borders(Borders::ALL).title(after_title))
        .wrap(ratatui::widgets::Wrap { trim: false });

    (before_p, after_p)
}
