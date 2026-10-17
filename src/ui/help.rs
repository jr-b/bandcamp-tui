//! The `?` help overlay: every key binding, grouped.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, Paragraph};

use super::ACCENT;
use crate::input::HELP;

const KEY_WIDTH: usize = 18;

pub(super) fn draw(frame: &mut Frame, area: Rect) {
    let mut lines = Vec::new();
    for (i, (section, keys)) in HELP.iter().enumerate() {
        if i > 0 {
            lines.push(Line::default());
        }
        lines.push(Line::from(*section).fg(ACCENT).bold());
        for (key, what) in *keys {
            lines.push(Line::from(vec![
                Span::styled(format!("  {key:<KEY_WIDTH$}"), Style::new().bold()),
                Span::raw(*what),
            ]));
        }
    }
    lines.push(Line::default());
    lines.push(Line::from("Any key closes this.").dim());

    let width = u16::try_from(
        lines
            .iter()
            .map(Line::width)
            .max()
            .unwrap_or(0)
            .saturating_add(4),
    )
    .unwrap_or(u16::MAX)
    .min(area.width);
    let height = u16::try_from(lines.len().saturating_add(2))
        .unwrap_or(u16::MAX)
        .min(area.height);
    let [popup] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(area);
    let [popup] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(popup);

    let block = Block::bordered()
        .title(" Keys ")
        .title_style(Style::new().bold())
        .border_style(Style::new().fg(ACCENT));
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(block.padding(ratatui::widgets::Padding::horizontal(1))),
        popup,
    );
}
