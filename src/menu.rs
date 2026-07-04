use std::{
    io::{IsTerminal, Write, stdin, stdout},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use crossterm::{
    cursor::{Hide, RestorePosition, SavePosition, Show},
    event::{Event, KeyCode, KeyEventKind, KeyModifiers, poll, read},
    execute, queue,
    style::{Attribute, Print, SetAttribute},
    terminal::{Clear, ClearType, disable_raw_mode, enable_raw_mode},
};

#[derive(Debug, Clone)]
pub struct MenuItem {
    pub label: String,
    pub description: Option<String>,
    pub details: Vec<String>,
}

impl MenuItem {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            description: None,
            details: Vec::new(),
        }
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.details.push(detail.into());
        self
    }
}

pub struct Menu {
    title: String,
    help: String,
    note: Option<String>,
    cancel_message: String,
    items: Vec<MenuItem>,
    selected: usize,
}

impl Menu {
    pub fn new(title: impl Into<String>, items: Vec<MenuItem>) -> Self {
        Self {
            title: title.into(),
            help: "Use Up/Down. Enter selects. Esc cancels.".to_string(),
            note: None,
            cancel_message: "menu canceled".to_string(),
            items,
            selected: 0,
        }
    }

    pub fn help(mut self, help: impl Into<String>) -> Self {
        self.help = help.into();
        self
    }

    pub fn note(mut self, note: Option<String>) -> Self {
        self.note = note;
        self
    }

    pub fn cancel_message(mut self, cancel_message: impl Into<String>) -> Self {
        self.cancel_message = cancel_message.into();
        self
    }

    pub fn select(self) -> Result<usize> {
        if self.items.is_empty() {
            bail!("menu requires at least one item");
        }

        if !stdin().is_terminal() || !stdout().is_terminal() {
            bail!("interactive menu requires a terminal");
        }

        let mut selected = self.selected;
        let mut stdout = stdout();
        let _guard = TerminalGuard::enter(&mut stdout)?;
        render(&mut stdout, &self, selected)?;

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
                KeyCode::Up => selected = selected.saturating_sub(1),
                KeyCode::Down if selected + 1 < self.items.len() => selected += 1,
                KeyCode::Home => selected = 0,
                KeyCode::End => selected = self.items.len() - 1,
                KeyCode::Enter => break,
                KeyCode::Esc => bail!(self.cancel_message),
                KeyCode::Char('c') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                    bail!(self.cancel_message)
                }
                _ => {}
            }

            render(&mut stdout, &self, selected)?;
        }

        Ok(selected)
    }
}

fn render(stdout: &mut std::io::Stdout, menu: &Menu, selected: usize) -> Result<()> {
    queue!(
        stdout,
        RestorePosition,
        Clear(ClearType::FromCursorDown),
        Print(format!("{}\n", menu.title)),
        Print(format!("{}\n\n", menu.help))
    )?;

    if let Some(note) = &menu.note {
        queue!(stdout, Print(format!("{note}\n\n")))?;
    }

    for (index, item) in menu.items.iter().enumerate() {
        if index == selected {
            queue!(stdout, SetAttribute(Attribute::Reverse), Print("> "))?;
        } else {
            queue!(stdout, Print("  "))?;
        }

        if let Some(description) = &item.description {
            queue!(stdout, Print(format!("{:<18} {}", item.label, description)))?;
        } else {
            queue!(stdout, Print(&item.label))?;
        }

        if index == selected {
            queue!(stdout, SetAttribute(Attribute::Reset))?;
        }

        queue!(stdout, Print("\n"))?;

        for detail in &item.details {
            queue!(stdout, Print(format!("    {detail}\n")))?;
        }
    }

    stdout.flush().context("failed to render menu")
}

struct TerminalGuard;

impl TerminalGuard {
    fn enter(stdout: &mut std::io::Stdout) -> Result<Self> {
        enable_raw_mode().context("failed to enable terminal raw mode")?;
        execute!(stdout, SavePosition, Hide).context("failed to prepare terminal")?;
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
    fn menu_items_can_hold_details() {
        let item = MenuItem::new("Chocolate")
            .description("Warm menu item")
            .detail("chip Black on DarkYellow");

        assert_eq!(item.label, "Chocolate");
        assert_eq!(item.description.as_deref(), Some("Warm menu item"));
        assert_eq!(item.details, vec!["chip Black on DarkYellow"]);
    }
}
