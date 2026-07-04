use std::{
    io::{IsTerminal, Write, stdin, stdout},
    time::Duration,
};

use anyhow::{Context, Result};
use crossterm::{
    cursor::{Hide, MoveTo, Show, position},
    event::{Event, KeyCode, KeyEventKind, KeyModifiers, poll, read},
    execute, queue,
    style::{Color, Print, ResetColor, SetBackgroundColor, SetForegroundColor},
    terminal::{Clear, ClearType, disable_raw_mode, enable_raw_mode, size},
};

const PAGE_SIZE: usize = 20;
const PAGE_SIZE_U16: u16 = 20;
const MENU_HEIGHT: u16 = 27;
const ROW_START: u16 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerItem {
    pub label: String,
    pub detail: String,
}

impl PickerItem {
    pub fn new(label: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            detail: detail.into(),
        }
    }
}

pub struct Picker {
    title: String,
    header: String,
    help: String,
    items: Vec<PickerItem>,
}

impl Picker {
    pub fn new(
        title: impl Into<String>,
        header: impl Into<String>,
        items: Vec<PickerItem>,
    ) -> Self {
        Self {
            title: title.into(),
            header: header.into(),
            help: "Use Up/Down. PageUp/PageDown changes pages. Enter selects. Esc closes."
                .to_string(),
            items,
        }
    }

    pub fn help(mut self, help: impl Into<String>) -> Self {
        self.help = help.into();
        self
    }

    pub fn select(&self) -> Result<Option<usize>> {
        if !stdin().is_terminal() || !stdout().is_terminal() {
            return Ok(None);
        }

        let mut stdout = stdout();
        let _guard = TerminalGuard::enter(&mut stdout)?;
        queue!(stdout, Print("\n"))?;
        stdout.flush()?;
        let (_, top) = position().context("failed to read cursor position")?;

        if self.items.is_empty() {
            render_empty(&mut stdout, top, self)?;
            wait_for_close()?;
            clear_region(&mut stdout, top)?;
            return Ok(None);
        }

        let mut page = 0usize;
        let mut selected = 0usize;
        render_full(&mut stdout, top, self, page, selected)?;

        loop {
            if !poll(Duration::from_millis(250)).context("failed to poll terminal input")? {
                continue;
            }

            let Event::Key(event) = read().context("failed to read terminal input")? else {
                continue;
            };

            if event.kind == KeyEventKind::Release {
                continue;
            }

            let old_selected = selected;
            let old_page = page;

            match event.code {
                KeyCode::Up => {
                    selected = selected.saturating_sub(1);
                }
                KeyCode::Down if selected + 1 < self.items.len() => {
                    selected += 1;
                }
                KeyCode::PageUp => {
                    selected = selected.saturating_sub(PAGE_SIZE);
                }
                KeyCode::PageDown => {
                    selected = (selected + PAGE_SIZE).min(self.items.len() - 1);
                }
                KeyCode::Home => selected = 0,
                KeyCode::End => selected = self.items.len() - 1,
                KeyCode::Enter => {
                    clear_region(&mut stdout, top)?;
                    return Ok(Some(selected));
                }
                KeyCode::Esc => {
                    clear_region(&mut stdout, top)?;
                    return Ok(None);
                }
                KeyCode::Char('c') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                    clear_region(&mut stdout, top)?;
                    return Ok(None);
                }
                _ => {}
            }

            page = selected / PAGE_SIZE;
            if page != old_page {
                render_full(&mut stdout, top, self, page, selected)?;
            } else if selected != old_selected {
                render_row(&mut stdout, top, self, page, old_selected, selected)?;
                render_row(&mut stdout, top, self, page, selected, selected)?;
                stdout.flush().context("failed to update menu")?;
            }
        }
    }
}

fn wait_for_close() -> Result<()> {
    loop {
        if !poll(Duration::from_millis(250)).context("failed to poll terminal input")? {
            continue;
        }

        let Event::Key(event) = read().context("failed to read terminal input")? else {
            continue;
        };

        if event.kind == KeyEventKind::Release {
            continue;
        }

        match event.code {
            KeyCode::Esc | KeyCode::Enter => return Ok(()),
            KeyCode::Char('c') if event.modifiers.contains(KeyModifiers::CONTROL) => return Ok(()),
            _ => {}
        }
    }
}

fn render_empty(stdout: &mut std::io::Stdout, top: u16, picker: &Picker) -> Result<()> {
    clear_region(stdout, top)?;
    queue!(
        stdout,
        MoveTo(0, top),
        SetForegroundColor(Color::DarkGrey),
        Print(&picker.title),
        ResetColor,
        MoveTo(0, top + 2),
        Print("No entries found."),
        MoveTo(0, top + 4),
        SetForegroundColor(Color::DarkGrey),
        Print("Press Esc to close."),
        ResetColor
    )?;
    stdout.flush().context("failed to render empty menu")
}

fn render_full(
    stdout: &mut std::io::Stdout,
    top: u16,
    picker: &Picker,
    page: usize,
    selected: usize,
) -> Result<()> {
    clear_region(stdout, top)?;
    queue!(
        stdout,
        MoveTo(0, top),
        SetForegroundColor(Color::DarkGrey),
        Print(&picker.title),
        ResetColor,
        MoveTo(0, top + 1),
        SetForegroundColor(Color::DarkGrey),
        Print(&picker.help),
        ResetColor,
        MoveTo(0, top + 2),
        SetForegroundColor(Color::DarkGrey),
        Print(&picker.header),
        ResetColor
    )?;

    for row in 0..PAGE_SIZE {
        render_row(stdout, top, picker, page, page * PAGE_SIZE + row, selected)?;
    }

    queue!(
        stdout,
        MoveTo(0, top + ROW_START + PAGE_SIZE_U16 + 1),
        SetForegroundColor(Color::DarkGrey),
        Print(format!(
            "Page {}/{}",
            page + 1,
            picker.items.len().max(1).div_ceil(PAGE_SIZE)
        )),
        ResetColor
    )?;

    render_row(stdout, top, picker, page, selected, selected)?;
    stdout.flush().context("failed to render menu")
}

fn render_row(
    stdout: &mut std::io::Stdout,
    top: u16,
    picker: &Picker,
    page: usize,
    index: usize,
    selected: usize,
) -> Result<()> {
    let row = index.saturating_sub(page * PAGE_SIZE);
    if row >= PAGE_SIZE {
        return Ok(());
    }

    let y = top + ROW_START + row as u16;
    let (width, _) = size().unwrap_or((100, 30));
    queue!(stdout, MoveTo(0, y), Clear(ClearType::CurrentLine))?;

    let Some(item) = picker.items.get(index) else {
        return Ok(());
    };

    let label_width = (usize::from(width) / 3).clamp(18, 34);
    let detail_width = usize::from(width).saturating_sub(label_width + 3).max(10);
    let line = format!(
        "{:<label_width$} {}",
        clamp(&item.label, label_width),
        clamp(&item.detail, detail_width),
        label_width = label_width
    );

    if index == selected {
        queue!(
            stdout,
            SetForegroundColor(Color::Black),
            SetBackgroundColor(Color::Yellow),
            Print(line),
            ResetColor
        )?;
    } else {
        queue!(stdout, Print(line))?;
    }

    Ok(())
}

fn clear_region(stdout: &mut std::io::Stdout, top: u16) -> Result<()> {
    for offset in 0..MENU_HEIGHT {
        queue!(
            stdout,
            MoveTo(0, top + offset),
            Clear(ClearType::CurrentLine)
        )?;
    }

    queue!(stdout, MoveTo(0, top))?;
    stdout.flush().context("failed to clear menu")
}

fn clamp(value: &str, max: usize) -> String {
    if max < 4 || value.chars().count() <= max {
        return value.to_string();
    }

    let mut text = value.chars().take(max - 3).collect::<String>();
    text.push_str("...");
    text
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter(stdout: &mut std::io::Stdout) -> Result<Self> {
        enable_raw_mode().context("failed to enable terminal raw mode")?;
        execute!(stdout, Hide).context("failed to prepare terminal")?;
        Ok(Self)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(stdout(), Show);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picker_item_stores_label_and_detail() {
        let item = PickerItem::new("PID 10", "pwsh");

        assert_eq!(item.label, "PID 10");
        assert_eq!(item.detail, "pwsh");
    }

    #[test]
    fn clamps_long_values() {
        assert_eq!(clamp("abcdef", 5), "ab...");
        assert_eq!(clamp("abc", 5), "abc");
    }
}
