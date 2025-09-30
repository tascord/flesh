use {
    crate::Message,
    itertools::{Itertools, repeat_n},
    ratatui::{
        layout::{Constraint, Direction, Layout, Margin},
        style::{Color, Style, Stylize},
        text::{Line, Span},
        widgets::{Block, Borders, Paragraph, StatefulWidget, Widget},
    },
    std::fmt::Display,
};

pub struct MessageList(Vec<Message>);
impl MessageList {
    pub fn new(m: Vec<Message>) -> Self { Self(m) } 
}

const COLOURS: &[Color] =
    &[Color::LightBlue, Color::LightCyan, Color::LightGreen, Color::LightMagenta, Color::LightRed, Color::LightYellow];

impl Into<Line<'_>> for Message {
    fn into(self) -> Line<'static> {
        match self {
            Message::Text { author, content } => Line::from_iter([
                Span::from(format!("{author}: "))
                    .bold()
                    .fg(COLOURS[author.as_bytes().iter().fold(0usize, |a, b| a as usize + *b as usize) % COLOURS.len()]),
                Span::from(content).fg(Color::White),
            ]),
            Message::Join(author) => Line::from_iter([
                Span::from(format!("{author} "))
                    .bold()
                    .fg(COLOURS[author.as_bytes().iter().fold(0usize, |a, b| a as usize + *b as usize) % COLOURS.len()]),
                Span::from("joins the room.").fg(Color::Gray),
            ]),
        }
    }
}

impl Widget for MessageList {
    fn render(self, area: ratatui::prelude::Rect, buf: &mut ratatui::prelude::Buffer)
    where
        Self: Sized,
    {
        let block = Block::default()
            .title("Messages")
            .borders(Borders::ALL)
            .border_style(ratatui::style::Style::default().fg(ratatui::style::Color::Yellow));

        let messages = self.0.iter().rev().take(area.height.saturating_sub(2).into()).collect_vec();
        let lay = Layout::new(Direction::Vertical, repeat_n(Constraint::Length(1), messages.len()))
            .split(area.inner(Margin::new(1, 1)));

        lay.into_iter().zip(messages.into_iter().rev()).for_each(|(a, b)| {
            Into::<Line>::into(b.clone()).render(*a, buf);
        });

        block.render(area, buf);
    }
}

pub struct Textbox {
    label: String,
}

impl Textbox {
    pub fn new(l: impl Display) -> Self { Self { label: l.to_string() } }
}
impl StatefulWidget for Textbox {
    type State = (String, usize);

    fn render(self, area: ratatui::prelude::Rect, buf: &mut ratatui::prelude::Buffer, state: &mut Self::State) {
        let (value, cursor) = state.clone();

        let mut offset = 0;
        let slice = match area.columns().count() < value.len() + 3 && cursor > area.columns().count() {
            false => value.clone(),
            true => {
                // slice with the cursor at the end
                offset = cursor - area.columns().count() + 3;
                value[offset..].to_string()
            }
        };

        let block = Block::default()
            .title(self.label)
            .borders(Borders::ALL)
            .border_style(ratatui::style::Style::default().fg(ratatui::style::Color::Yellow));

        Paragraph::new(add_cursor(slice, cursor - offset)).block(block).render(area, buf);
    }
}

pub fn add_cursor<'a>(s: String, c: usize) -> Line<'a> {
    Line::from(vec![
        Span::raw(s[..c].to_string()),
        Span::styled(s.clone().chars().nth(c).unwrap_or(' ').to_string(), Style::new().bg(Color::Yellow)),
        Span::raw(match c == s.len() {
            true => String::new(),
            false => s[c + 1..].to_string(),
        }),
    ])
}
