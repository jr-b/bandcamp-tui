//! The filter picker popup.

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem};

use super::theme::theme;
use crate::picker::Picker;

pub(super) fn draw(frame: &mut Frame, area: Rect, picker: &mut Picker) {
    let width = area.width.saturating_sub(4).clamp(20, 56);
    let wanted_height = u16::try_from(picker.visible_len())
        .unwrap_or(u16::MAX)
        .saturating_add(4);
    let height = wanted_height.clamp(6, area.height.saturating_sub(2).max(6));
    let [popup] = Layout::horizontal([Constraint::Length(width)])
        .flex(Flex::Center)
        .areas(area);
    let [popup] = Layout::vertical([Constraint::Length(height)])
        .flex(Flex::Center)
        .areas(popup);

    let block = Block::bordered()
        .title(format!(" {} ", picker.kind.title()))
        .title_style(Style::new().bold())
        .border_style(theme().accent);
    let inner = block.inner(popup);
    frame.render_widget(Clear, popup);
    frame.render_widget(block, popup);

    let [input, rule, list] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .areas(inner);
    super::draw_input(frame, input, &picker.query, "type to search");
    frame.render_widget(Line::from("─".repeat(usize::from(rule.width))).dim(), rule);

    let searching = picker.is_searching();
    let items: Vec<ListItem> = picker
        .visible()
        .map(|item| {
            let indent = if item.nested && !searching { "  " } else { "" };
            let mut spans = vec![Span::raw(format!("{indent}{}", item.label))];
            // When searching, show which genre a subgenre belongs to.
            if (searching || item.nested && indent.is_empty())
                && let Some(detail) = &item.detail
            {
                spans.push(Span::styled(format!("  {detail}"), Style::new().dim()));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    if items.is_empty() {
        frame.render_widget(Line::from("No matches.").dim(), list);
        return;
    }
    let list_widget = List::new(items)
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED))
        .highlight_symbol("› ");
    frame.render_stateful_widget(list_widget, list, &mut picker.list);
}
