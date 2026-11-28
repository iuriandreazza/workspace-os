//! Draws the state with `ratatui`. All content is text from the state; this
//! only decides where it goes and how it is coloured.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block as Frame_, Clear, Paragraph};

use super::state::{Kind, Mode, ROOT_KEYS, State};
use super::wrap::{tail, text_width, wrap};
use crate::ports::ChoiceLine;

const PLAN_KEYS: &str = "[y] run the steps marked ok   [d] details   [n] cancel";
const CHOOSE_KEYS: &str = "↑/↓ select · Enter choose · Esc cancel";
const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

fn style_of(kind: Kind) -> Style {
    match kind {
        Kind::Request => Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
        Kind::Note => Style::new().fg(Color::DarkGray),
        Kind::Plan | Kind::Outcome => Style::new(),
        Kind::Reply => Style::new().add_modifier(Modifier::ITALIC),
        Kind::Root => Style::new().fg(Color::Red),
    }
}

fn dim(text: String) -> Line<'static> {
    Line::styled(text, Style::new().fg(Color::DarkGray))
}

/// The lines under the history, for every mode but a root confirmation.
fn bottom(state: &State, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![dim("─".repeat(width))];
    match &state.mode {
        Mode::Input => {
            lines.push(Line::from(format!(
                "▸ {}",
                tail(&state.input, width.saturating_sub(3))
            )));
            lines.push(dim(state.input_hint().to_owned()));
        }
        Mode::Busy { label, since_ms } => {
            let frame = SPINNER[(state.clock_ms / 100) as usize % SPINNER.len()];
            let seconds = state.clock_ms.saturating_sub(*since_ms) / 1000;
            lines.push(Line::from(format!("{frame} {label}… {seconds}s")));
            lines.push(dim(if *label == "listening" {
                "Enter or ^T stop (only the first 30 s are kept) · Esc cancel · ^C quit".to_owned()
            } else if *label == "transcribing" {
                "gives up after 30 s · ^C quit".to_owned()
            } else {
                "PgUp/Dn scroll · ^C quit".to_owned()
            }));
        }
        Mode::Plan { .. } => lines.extend(
            wrap(PLAN_KEYS, width)
                .into_iter()
                .map(|row| Line::styled(row, Style::new().add_modifier(Modifier::BOLD))),
        ),
        Mode::Choose {
            lines: choices,
            selected,
        } => {
            lines.extend(option_lines(
                choices,
                *selected,
                width,
                usize::from(state.height / 2).min(usize::from(state.height).saturating_sub(4)),
            ));
            lines.extend(wrap(CHOOSE_KEYS, width).into_iter().map(dim));
        }
        Mode::Root { .. } => {}
    }
    lines
}

/// The tree lines of a choice, the selected candidate marked and scrolled into view.
fn option_lines(
    choices: &[ChoiceLine],
    selected: usize,
    width: usize,
    room: usize,
) -> Vec<Line<'static>> {
    let room = room.max(1);
    let mut lines = Vec::new();
    let mut selected_first = 0;
    for choice in choices {
        let is_selected = choice.option == Some(selected);
        if is_selected {
            selected_first = lines.len();
        }
        let style = if is_selected {
            Style::new().add_modifier(Modifier::REVERSED)
        } else {
            Style::new()
        };
        let room_for_text = width.saturating_sub(2);
        for (row, text) in wrap(
            &crate::screen::fit_choice(&choice.text, room_for_text, super::wrap::cell_width),
            room_for_text,
        )
        .into_iter()
        .enumerate()
        {
            let marker = if is_selected && row == 0 {
                "▸ "
            } else {
                "  "
            };
            lines.push(Line::styled(format!("{marker}{text}"), style));
        }
    }
    let start = (selected_first + 1)
        .saturating_sub(room)
        .min(selected_first);
    lines.into_iter().skip(start).take(room).collect()
}

fn root_modal(frame: &mut Frame, state: &State, panel: &str) {
    let screen = frame.area();
    let columns = usize::from(screen.width.saturating_sub(2));
    let key_rows = wrap(ROOT_KEYS, columns).len();
    // As tall as the action needs, at the bottom, with the history above it;
    // the whole screen when it does not fit.
    let wanted = wrap(panel, columns).len() + key_rows + 2;
    let height = if state.root_fits() {
        wanted.min(usize::from(screen.height)) as u16
    } else {
        screen.height
    };
    let area = Rect::new(screen.x, screen.bottom() - height, screen.width, height);
    frame.render_widget(Clear, area);
    let outline = Frame_::bordered()
        .title(" ROOT ACTION ")
        .border_style(Style::new().fg(Color::Red));
    let inner = outline.inner(area);
    frame.render_widget(outline, area);
    let key_rows = key_rows as u16;
    let [content, keys] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(key_rows)]).areas(inner);
    if state.root_fits() {
        let text = wrap(panel, usize::from(inner.width)).join("\n");
        frame.render_widget(Paragraph::new(text), content);
        frame.render_widget(
            Paragraph::new(wrap(ROOT_KEYS, usize::from(inner.width)).join("\n"))
                .style(Style::new().add_modifier(Modifier::BOLD)),
            keys,
        );
    } else {
        // Showing part of an action to be confirmed would defeat ADR 0007.
        frame.render_widget(
            Paragraph::new("The terminal is too small to show the whole action. Enlarge it, or press n to refuse.")
                .wrap(ratatui::widgets::Wrap { trim: false }),
            content,
        );
        frame.render_widget(Paragraph::new("[n] refuse"), keys);
    }
}

pub fn draw(frame: &mut Frame, state: &State) {
    let area = frame.area();
    let width = usize::from(area.width);
    let header = wrap(&state.header, width);
    let header_height = header.len().min(usize::from(area.height / 3));
    let bottom_lines = bottom(state, width);
    let bottom_height = bottom_lines
        .len()
        .min(usize::from(area.height.saturating_sub(1)));
    let [top, middle, low] = Layout::vertical([
        Constraint::Length(header_height as u16),
        Constraint::Min(0),
        Constraint::Length(bottom_height as u16),
    ])
    .areas(area);
    frame.render_widget(
        Paragraph::new(header[..header_height].join("\n")).style(Style::new().fg(Color::DarkGray)),
        top,
    );
    let history: Vec<Line> = state
        .visible_history(width, usize::from(middle.height))
        .into_iter()
        .map(|(kind, text)| Line::styled(text, style_of(kind)))
        .collect();
    frame.render_widget(Paragraph::new(Text::from(history)), middle);
    frame.render_widget(Paragraph::new(Text::from(bottom_lines)), low);
    if let Mode::Root { panel } = &state.mode {
        root_modal(frame, state, panel);
    }
    if state.mode == Mode::Input && low.height > 1 {
        let typed = tail(&state.input, width.saturating_sub(3));
        frame.set_cursor_position(Position::new(
            (2 + text_width(&typed)).min(width.saturating_sub(1)) as u16,
            low.y + 1,
        ));
    }
}

#[cfg(test)]
mod tests {
    use crate::ports::ChoiceLine;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::draw;
    use crate::tui::state::{Event, State};

    fn render(state: &State, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| draw(frame, state)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }

    fn state_with(blocks: &[&str]) -> State {
        let mut state = State::new("HEADER user".to_owned());
        state.set_size(40, 12);
        for block in blocks {
            state.on_event(Event::Outcome((*block).to_owned()));
        }
        state
    }

    #[test]
    fn the_header_is_on_top_the_input_at_the_bottom_and_history_between() {
        let state = state_with(&["first answer", "second answer"]);
        let screen = render(&state, 40, 12);
        assert_eq!(screen[0], "HEADER user");
        assert!(screen[1].starts_with("first answer"), "{screen:#?}");
        assert!(screen[3].starts_with("second answer"), "{screen:#?}");
        assert!(screen[10].starts_with('▸'), "{screen:#?}");
        assert!(screen[11].contains("^D quit"));
        assert!(screen[9].starts_with('─'));
    }

    #[test]
    fn a_long_history_shows_its_end_and_scrolls() {
        let text: String = (1..=40).map(|n| format!("line {n}\n")).collect();
        let mut state = state_with(&[&text]);
        let screen = render(&state, 40, 12);
        assert!(screen.iter().any(|l| l == "line 40"));
        assert!(!screen.iter().any(|l| l == "line 1"));
        state.scroll = 1000;
        let screen = render(&state, 40, 12);
        assert!(screen.iter().any(|l| l == "line 1"));
        assert!(!screen.iter().any(|l| l == "line 40"));
    }

    #[test]
    fn the_typed_request_is_shown_and_its_end_stays_in_view() {
        let mut state = state_with(&[]);
        state.input = "x".repeat(60) + "END";
        let screen = render(&state, 40, 12);
        assert!(screen[10].ends_with("END"), "{screen:#?}");
        assert!(screen[10].len() <= 40 + 4);
    }

    #[test]
    fn the_wait_shows_what_the_shell_is_doing_and_for_how_long() {
        let mut state = state_with(&[]);
        state.on_event(Event::Busy("asking the model"));
        state.clock_ms = 3200;
        let screen = render(&state, 40, 12);
        assert!(screen[10].contains("asking the model… 3s"), "{screen:#?}");
    }

    #[test]
    fn the_plan_bar_lists_the_keys() {
        let mut state = state_with(&[]);
        state.on_event(Event::AskPlan {
            screen: "PLAN TABLE".to_owned(),
            details: String::new(),
        });
        let screen = render(&state, 60, 12);
        assert!(screen.iter().any(|l| l == "PLAN TABLE"));
        assert!(screen[11].contains("[y] run the steps marked ok   [d] details   [n] cancel"));
    }

    #[test]
    fn the_key_bars_wrap_on_a_narrow_terminal_instead_of_being_cut() {
        let mut state = state_with(&[]);
        state.on_event(Event::AskPlan {
            screen: "PLAN".to_owned(),
            details: String::new(),
        });
        let screen = render(&state, 24, 12);
        let all = screen.join(" ");
        assert!(
            all.contains("[y] run") && all.contains("[n] cancel"),
            "{screen:#?}"
        );
    }

    #[test]
    fn the_choice_marks_the_selected_option_and_follows_it() {
        let mut state = state_with(&[]);
        state.on_event(Event::AskChoose {
            question: "Which?".to_owned(),
            lines: (1..=20)
                .map(|n| ChoiceLine {
                    text: format!("choice {n}"),
                    option: Some(n - 1),
                    label: String::new(),
                })
                .collect(),
        });
        state.clock_ms = crate::tui::state::DWELL_MS;
        let screen = render(&state, 40, 12);
        assert!(screen.iter().any(|l| l == "▸ choice 1"), "{screen:#?}");
        for _ in 0..15 {
            state.clock_ms += crate::tui::state::DWELL_MS;
            state.on_key(ratatui::crossterm::event::KeyEvent::from(
                ratatui::crossterm::event::KeyCode::Down,
            ));
        }
        let screen = render(&state, 40, 12);
        assert!(screen.iter().any(|l| l == "▸ choice 16"), "{screen:#?}");
    }

    #[test]
    fn a_choice_shows_its_context_unmarked_and_marks_only_the_selected_candidate() {
        let line = |text: &str, option| ChoiceLine {
            text: text.to_owned(),
            option,
            label: text.trim().to_owned(),
        };
        let mut state = state_with(&[]);
        state.on_event(Event::AskChoose {
            question: "Which?".to_owned(),
            lines: vec![
                line("~  2 folders", None),
                line("├─ Documents/", None),
                line("│  └─ report.pdf", Some(0)),
                line("└─ Downloads/", None),
                line("   └─ report.pdf", Some(1)),
            ],
        });
        state.clock_ms = crate::tui::state::DWELL_MS;
        let screen = render(&state, 40, 14);
        assert!(screen.iter().any(|l| l == "  ├─ Documents/"), "{screen:#?}");
        assert!(
            screen.iter().any(|l| l == "▸ │  └─ report.pdf"),
            "{screen:#?}"
        );
        assert!(
            screen.iter().any(|l| l == "     └─ report.pdf"),
            "{screen:#?}"
        );
    }

    #[test]
    fn the_root_panel_is_separate_and_shows_the_whole_real_location() {
        let mut state = state_with(&["earlier answer"]);
        let path = "/Users/someone/very/long/real/location/of/the/file.iso";
        state.on_event(Event::AskRoot {
            panel: format!("ROOT s2 fs.delete {path}"),
        });
        state.set_size(30, 12);
        let screen = render(&state, 30, 12);
        let top = screen
            .iter()
            .position(|l| l.contains("ROOT ACTION"))
            .expect("the panel has a title");
        assert!(top > 1, "the history stays visible above it: {screen:#?}");
        assert!(
            screen[..top]
                .iter()
                .any(|l| l.starts_with("earlier answer"))
        );
        let shown: String = screen[top + 1..11]
            .iter()
            .map(|l| l.trim_matches('│').trim())
            .collect();
        assert!(
            shown.replace(' ', "").contains(&path.replace(' ', "")),
            "{screen:#?}"
        );
        let keys: String = screen[top + 1..11].concat();
        assert!(
            keys.contains("[y] confirm this exact") && keys.contains("[n] refuse"),
            "{screen:#?}"
        );
        assert!(screen[11].starts_with('└'));
    }

    #[test]
    fn a_root_panel_that_cannot_fit_says_so_and_offers_only_refusal() {
        let mut state = state_with(&[]);
        state.on_event(Event::AskRoot {
            panel: "x".repeat(2000),
        });
        state.set_size(30, 8);
        let screen = render(&state, 30, 8);
        let all = screen.join(" ");
        assert!(all.contains("too small"), "{screen:#?}");
        assert!(!all.contains("[y]"));
    }

    #[test]
    fn a_tiny_terminal_does_not_panic() {
        for (w, h) in [(1, 1), (5, 3), (10, 2)] {
            let mut state = state_with(&["answer"]);
            state.on_event(Event::AskPlan {
                screen: "p".to_owned(),
                details: String::new(),
            });
            state.set_size(w, h);
            render(&state, w, h);
        }
    }
}
