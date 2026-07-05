use std::{
    io::{Write, stdout},
    sync::mpsc::{self, TryRecvError},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use crossterm::{
    cursor::{Hide, MoveTo, Show, position},
    event::{Event, KeyCode, KeyEventKind, KeyModifiers, poll, read},
    execute, queue,
    style::{Color, Print, ResetColor, SetForegroundColor},
    terminal::{Clear, ClearType, disable_raw_mode, enable_raw_mode},
};

const LOADING_FRAMES: [&str; 6] = ["[.  ]", "[.. ]", "[...]", "[.. ]", "[.  ]", "[   ]"];
const LOADING_HEIGHT: u16 = 7;

pub fn load_with_terminal<T, F>(
    title: &str,
    loading_text: &str,
    prepare_text: &str,
    cancel_message: &str,
    load: F,
) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T> + Send + 'static,
{
    let mut stdout = stdout();
    let _guard = TerminalGuard::enter(&mut stdout)?;
    queue!(stdout, Print("\n"))?;
    stdout.flush()?;
    let (_, top) = position().context("failed to read cursor position")?;

    clear_region(&mut stdout, top, LOADING_HEIGHT)?;
    render_shell(&mut stdout, top, title, prepare_text)?;

    let (sender, receiver) = mpsc::channel();
    let _loader = thread::spawn(move || {
        let _ = sender.send(load());
    });

    let mut frame = 0usize;
    loop {
        render_frame(&mut stdout, top, loading_text, frame)?;

        match receiver.try_recv() {
            Ok(Ok(value)) => {
                clear_region(&mut stdout, top, LOADING_HEIGHT)?;
                return Ok(value);
            }
            Ok(Err(error)) => {
                clear_region(&mut stdout, top, LOADING_HEIGHT)?;
                return Err(error);
            }
            Err(TryRecvError::Disconnected) => {
                clear_region(&mut stdout, top, LOADING_HEIGHT)?;
                bail!("failed to load data");
            }
            Err(TryRecvError::Empty) => {}
        }

        if poll(Duration::from_millis(140)).context("failed to poll terminal input")? {
            let Event::Key(event) = read().context("failed to read terminal input")? else {
                continue;
            };

            if event.kind == KeyEventKind::Release {
                continue;
            }

            if event.code == KeyCode::Esc
                || (event.code == KeyCode::Char('c')
                    && event.modifiers.contains(KeyModifiers::CONTROL))
            {
                clear_region(&mut stdout, top, LOADING_HEIGHT)?;
                bail!("{cancel_message}");
            }
        }

        frame = frame.wrapping_add(1);
    }
}

pub fn render_shell(
    stdout: &mut std::io::Stdout,
    top: u16,
    title: &str,
    prepare_text: &str,
) -> Result<()> {
    queue!(
        stdout,
        MoveTo(0, top),
        SetForegroundColor(Color::DarkGrey),
        Print(title),
        ResetColor,
        MoveTo(0, top + 4),
        SetForegroundColor(Color::DarkGrey),
        Print(prepare_text),
        ResetColor
    )?;

    stdout.flush().context("failed to render loading state")
}

pub fn render_frame(
    stdout: &mut std::io::Stdout,
    top: u16,
    loading_text: &str,
    frame_index: usize,
) -> Result<()> {
    queue!(
        stdout,
        MoveTo(0, top + 2),
        Clear(ClearType::CurrentLine),
        SetForegroundColor(Color::Yellow),
        Print(format!("{loading_text} {}", frame(frame_index))),
        ResetColor
    )?;

    stdout.flush().context("failed to render loading frame")
}

pub fn clear_region(stdout: &mut std::io::Stdout, top: u16, height: u16) -> Result<()> {
    for offset in 0..height {
        queue!(
            stdout,
            MoveTo(0, top + offset),
            Clear(ClearType::CurrentLine)
        )?;
    }

    queue!(stdout, MoveTo(0, top))?;
    stdout.flush().context("failed to clear loading region")
}

pub fn frame(frame_index: usize) -> &'static str {
    LOADING_FRAMES[frame_index % LOADING_FRAMES.len()]
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
    fn loading_frame_cycles() {
        assert_eq!(frame(0), "[.  ]");
        assert_eq!(frame(1), "[.. ]");
        assert_eq!(frame(2), "[...]");
        assert_eq!(frame(6), "[.  ]");
    }
}
