//! The `?` help overlay: every key binding, grouped. Two columns when the terminal is wide
//! enough; scrollable when it's too short.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Padding, Paragraph};

use super::theme::theme;
use crate::app::HelpView;
use crate::input::HELP;

const KEY_WIDTH: usize = 18;
/// Columns between the two halves.
const GAP: u16 = 3;
/// Border and padding on each side.
const CHROME_COLS: u16 = 4;

/// One section's lines: heading, then one line per binding.
fn section_lines(
    section: &'static str,
    keys: &[(&'static str, &'static str)],
) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(section).patch_style(theme().accent).bold()];
    for (key, what) in keys {
        lines.push(Line::from(vec![
            Span::styled(format!("  {key:<KEY_WIDTH$}"), Style::new().bold()),
            Span::raw(*what),
        ]));
    }
    lines
}

/// Stacks sections with a blank line between them.
fn stack(sections: &[Vec<Line<'static>>]) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (i, section) in sections.iter().enumerate() {
        if i > 0 {
            lines.push(Line::default());
        }
        lines.extend(section.iter().cloned());
    }
    lines
}

fn width_of(lines: &[Line<'_>]) -> u16 {
    u16::try_from(lines.iter().map(Line::width).max().unwrap_or(0)).unwrap_or(u16::MAX)
}

fn len_u16(n: usize) -> u16 {
    u16::try_from(n).unwrap_or(u16::MAX)
}

/// The help laid out in one column, or two when they fit in `max_width`.
fn columns(max_width: u16) -> Vec<Vec<Line<'static>>> {
    let sections: Vec<_> = HELP
        .iter()
        .map(|(section, keys)| section_lines(section, keys))
        .collect();
    let single = stack(&sections);
    // Split where the first half reaches about half the lines, keeping sections whole.
    let total: usize = sections.iter().map(Vec::len).sum();
    let mut split = 1;
    let mut first = sections[0].len();
    while split < sections.len() - 1 && first + sections[split].len() <= total.div_ceil(2) {
        first += sections[split].len();
        split += 1;
    }
    let (left, right) = (stack(&sections[..split]), stack(&sections[split..]));
    let two_wide = width_of(&left)
        .saturating_add(GAP)
        .saturating_add(width_of(&right))
        .saturating_add(CHROME_COLS);
    if sections.len() > 1 && two_wide <= max_width {
        vec![left, right]
    } else {
        vec![single]
    }
}

pub(super) fn draw(frame: &mut Frame, area: Rect, view: &mut HelpView) {
    let columns = columns(area.width);
    let text_rows = len_u16(columns.iter().map(Vec::len).max().unwrap_or(0));
    let text_cols = columns
        .iter()
        .map(|c| width_of(c))
        .fold(0u16, u16::saturating_add)
        .saturating_add(GAP * len_u16(columns.len() - 1));

    // Text, a blank line, the closing hint, and the border.
    let width = text_cols.saturating_add(CHROME_COLS).min(area.width);
    let height = text_rows.saturating_add(4).min(area.height);
    let [popup] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(area);
    let [popup] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(popup);

    let block = Block::bordered()
        .title(" Keys ")
        .title_style(Style::new().bold())
        .border_style(theme().accent)
        .padding(Padding::horizontal(1));
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);
    let [text, _, hint] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);

    view.page_rows = usize::from(text.height);
    let max_scroll = text_rows.saturating_sub(text.height);
    view.scroll = view.scroll.min(max_scroll);
    let hint_text = if max_scroll == 0 {
        "esc closes this.".to_owned()
    } else {
        let shown = (text.height + view.scroll).min(text_rows);
        format!("↑↓ scroll ({shown}/{text_rows})  esc closes")
    };
    frame.render_widget(Line::from(hint_text).dim(), hint);

    let constraints = columns
        .iter()
        .enumerate()
        .flat_map(|(i, c)| {
            let gap = (i > 0).then_some(Constraint::Length(GAP));
            gap.into_iter().chain([Constraint::Length(width_of(c))])
        })
        .collect::<Vec<_>>();
    let areas = Layout::horizontal(constraints).split(text);
    for (column, area) in columns.into_iter().zip(areas.iter().step_by(2)) {
        frame.render_widget(Paragraph::new(column).scroll((view.scroll, 0)), *area);
    }
}
