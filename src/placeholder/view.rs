use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Padding, Paragraph};

pub const FOOTER: &str = "Enter / Space / click  resume      q  shell";
pub const EXCERPT_LINES: usize = 10;

const MAX_WIDTH: u16 = 84;
const FRAME_COLUMNS: u16 = 4;
const FIXED_ROWS: u16 = 7;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewModel {
    pub name: String,
    pub cwd_display: String,
    pub conv_short: String,
    pub idle: String,
    pub parked_at: Option<String>,
    pub excerpt: String,
}

pub fn render(frame: &mut Frame, model: &ViewModel) {
    let area = frame.area();
    let width = area.width.min(MAX_WIDTH);
    let inner_width = width.saturating_sub(FRAME_COLUMNS);
    let footer_width = FOOTER.chars().count() as u16;
    if area.height < FIXED_ROWS || inner_width < footer_width {
        render_compact(frame, model);
        return;
    }
    let room = usize::from(area.height - FIXED_ROWS).saturating_sub(1);
    let excerpt = excerpt_lines(
        &model.excerpt,
        usize::from(inner_width),
        room.min(EXCERPT_LINES),
    );
    let mut lines = Vec::new();
    lines.push(title_line(model));
    lines.push(Line::from(model.cwd_display.clone()));
    lines.push(Line::from(meta_text(model)));
    if !excerpt.is_empty() {
        lines.push(Line::default());
        for text in excerpt {
            lines.push(Line::from(text));
        }
    }
    lines.push(Line::default());
    lines.push(footer_line());
    let height = lines.len() as u16 + 2;
    let block = Block::bordered().padding(Padding::horizontal(1));
    let paragraph = Paragraph::new(lines).block(block);
    frame.render_widget(paragraph, centered(area, width, height));
}

fn render_compact(frame: &mut Frame, model: &ViewModel) {
    let area = frame.area();
    let lines = vec![title_line(model), footer_line()];
    let height = area.height.min(lines.len() as u16);
    frame.render_widget(Paragraph::new(lines), centered(area, area.width, height));
}

fn title_line(model: &ViewModel) -> Line<'static> {
    let style = Style::default().add_modifier(Modifier::BOLD);
    Line::styled(format!("▶  {}", model.name), style)
}

fn footer_line() -> Line<'static> {
    let style = Style::default().add_modifier(Modifier::DIM);
    Line::styled(FOOTER, style)
}

fn meta_text(model: &ViewModel) -> String {
    let ViewModel {
        name: _,
        cwd_display: _,
        conv_short,
        idle,
        parked_at,
        excerpt: _,
    } = model;
    let text = format!("conv {conv_short}   idle {idle}");
    let Some(parked_at) = parked_at else {
        return text;
    };
    format!("{text}   parked {parked_at}")
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

pub fn excerpt_lines(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    let mut lines = Vec::new();
    if width == 0 || max_lines == 0 {
        return lines;
    }
    for line in text.trim().lines() {
        wrap_line(line, width, &mut lines);
        if lines.len() >= max_lines {
            break;
        }
    }
    lines.truncate(max_lines);
    lines
}

fn wrap_line(line: &str, width: usize, lines: &mut Vec<String>) {
    let mut current = String::new();
    let mut current_len = 0;
    for word in line.split_whitespace() {
        let word_len = word.chars().count();
        if current_len > 0 && current_len + 1 + word_len <= width {
            current.push(' ');
            current.push_str(word);
            current_len += 1 + word_len;
            continue;
        }
        if current_len > 0 {
            lines.push(std::mem::take(&mut current));
            current_len = 0;
        }
        for ch in word.chars() {
            if ch.is_control() {
                continue;
            }
            if current_len == width {
                lines.push(std::mem::take(&mut current));
                current_len = 0;
            }
            current.push(ch);
            current_len += 1;
        }
    }
    lines.push(current);
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;

    fn model(excerpt: &str) -> ViewModel {
        ViewModel {
            name: "siesta work".to_string(),
            cwd_display: "~/Projects/claude-siesta".to_string(),
            conv_short: "c0acdbe6".to_string(),
            idle: "26h 12m".to_string(),
            parked_at: Some("2026-09-27 12:40".to_string()),
            excerpt: excerpt.to_string(),
        }
    }

    fn screen(model: &ViewModel, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(frame, model)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut rows = Vec::new();
        for y in 0..height {
            let mut row = String::new();
            for x in 0..width {
                row.push_str(buffer[(x, y)].symbol());
            }
            rows.push(row);
        }
        rows
    }

    fn contains(rows: &[String], needle: &str) -> bool {
        for row in rows {
            if row.contains(needle) {
                return true;
            }
        }
        false
    }

    fn numbered(count: usize) -> String {
        let mut text = String::new();
        for index in 1..=count {
            text.push_str(&format!("line{index:02}\n"));
        }
        text
    }

    #[test]
    fn normal_pane_shows_every_part() {
        let rows = screen(&model("The fix is in.\n\nNext: tests."), 100, 30);
        assert!(contains(&rows, "▶  siesta work"));
        assert!(contains(&rows, "~/Projects/claude-siesta"));
        assert!(contains(
            &rows,
            "conv c0acdbe6   idle 26h 12m   parked 2026-09-27 12:40"
        ));
        assert!(contains(&rows, "The fix is in."));
        assert!(contains(&rows, "Next: tests."));
        assert!(contains(&rows, FOOTER));
    }

    #[test]
    fn parked_is_omitted_without_state() {
        let model = ViewModel {
            parked_at: None,
            ..model("hello")
        };
        let rows = screen(&model, 100, 30);
        assert!(contains(&rows, "conv c0acdbe6   idle 26h 12m"));
        assert!(!contains(&rows, "parked"));
    }

    #[test]
    fn excerpt_is_cut_at_ten_lines() {
        let rows = screen(&model(&numbered(15)), 100, 40);
        assert!(contains(&rows, "line01"));
        assert!(contains(&rows, "line10"));
        assert!(!contains(&rows, "line11"));
        assert!(contains(&rows, FOOTER));
    }

    #[test]
    fn short_pane_shrinks_the_excerpt() {
        let rows = screen(&model(&numbered(15)), 100, 12);
        assert!(contains(&rows, "line01"));
        assert!(contains(&rows, "line04"));
        assert!(!contains(&rows, "line05"));
        assert!(contains(&rows, "idle 26h 12m"));
        assert!(contains(&rows, FOOTER));
    }

    #[test]
    fn tiny_pane_shows_title_and_footer_only() {
        let rows = screen(&model("hidden excerpt"), 60, 4);
        assert!(contains(&rows, "▶  siesta work"));
        assert!(contains(&rows, FOOTER));
        assert!(!contains(&rows, "idle"));
        assert!(!contains(&rows, "hidden excerpt"));
    }

    #[test]
    fn narrow_pane_is_compact() {
        let rows = screen(&model("hidden excerpt"), 30, 30);
        assert!(contains(&rows, "▶  siesta work"));
        assert!(!contains(&rows, "idle"));
    }

    #[test]
    fn one_row_pane_does_not_panic() {
        let rows = screen(&model("x"), 10, 1);
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn wraps_on_words() {
        assert_eq!(
            excerpt_lines("alpha beta gamma delta", 11, 10),
            vec!["alpha beta", "gamma delta"]
        );
    }

    #[test]
    fn wraps_cyrillic_by_chars() {
        let lines = excerpt_lines("Привет мир, это проверка переноса", 12, 10);
        assert_eq!(lines, vec!["Привет мир,", "это проверка", "переноса"]);
        for line in &lines {
            assert!(line.chars().count() <= 12, "{line}");
        }
    }

    #[test]
    fn breaks_long_words() {
        assert_eq!(
            excerpt_lines("ab Абвгдеёжзий", 4, 10),
            vec!["ab", "Абвг", "деёж", "зий"]
        );
    }

    #[test]
    fn keeps_blank_lines_and_limits_count() {
        assert_eq!(excerpt_lines("one\n\ntwo", 10, 10), vec!["one", "", "two"]);
        assert_eq!(excerpt_lines(&numbered(5), 10, 3).len(), 3);
    }

    #[test]
    fn drops_control_characters() {
        assert_eq!(excerpt_lines("a\u{1b}[31mb", 20, 10), vec!["a[31mb"]);
    }

    #[test]
    fn empty_inputs() {
        assert!(excerpt_lines("", 10, 10).is_empty());
        assert!(excerpt_lines("text", 0, 10).is_empty());
        assert!(excerpt_lines("text", 10, 0).is_empty());
    }
}
