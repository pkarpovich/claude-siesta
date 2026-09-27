use std::io::{self, Stdout};
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitCode};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use nix::sys::signal::{SaFlags, SigAction, SigHandler, SigSet, Signal, sigaction};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::widgets::{Block, Paragraph};

static RESUME: AtomicBool = AtomicBool::new(false);

extern "C" fn on_usr1(_: nix::libc::c_int) {
    RESUME.store(true, Ordering::SeqCst);
}

enum Trigger {
    Key,
    Mouse,
    Signal,
}

enum Outcome {
    Resume(Trigger),
    Quit,
}

fn main() -> ExitCode {
    let mut program = Vec::new();
    for arg in std::env::args().skip(1) {
        program.push(arg);
    }
    let Some((bin, args)) = program.split_first() else {
        eprintln!("usage: spike <program> [args...]");
        return ExitCode::from(2);
    };

    let action = SigAction::new(
        SigHandler::Handler(on_usr1),
        SaFlags::SA_RESTART,
        SigSet::empty(),
    );
    if let Err(error) = unsafe { sigaction(Signal::SIGUSR1, &action) } {
        eprintln!("spike: sigaction: {error}");
        return ExitCode::from(1);
    }

    let mut terminal = match setup() {
        Ok(terminal) => terminal,
        Err(error) => {
            eprintln!("spike: terminal setup: {error}");
            return ExitCode::from(1);
        }
    };
    let outcome = event_loop(&mut terminal);
    restore();

    let trigger = match outcome {
        Ok(Outcome::Resume(trigger)) => trigger,
        Ok(Outcome::Quit) => return ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("spike: {error}");
            return ExitCode::from(1);
        }
    };
    let trigger = match trigger {
        Trigger::Key => "key",
        Trigger::Mouse => "mouse",
        Trigger::Signal => "signal",
    };
    println!("spike: resume via {trigger}");
    let error = Command::new(bin).args(args).exec();
    eprintln!("spike: exec {bin}: {error}");
    ExitCode::from(1)
}

fn setup() -> io::Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    execute!(io::stdout(), EnterAlternateScreen, EnableMouseCapture)?;
    Terminal::new(CrosstermBackend::new(io::stdout()))
}

fn restore() {
    let _ = execute!(io::stdout(), DisableMouseCapture, LeaveAlternateScreen);
    let _ = disable_raw_mode();
}

fn event_loop(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<Outcome> {
    let pid = std::process::id();
    let mut last = String::from("none");
    loop {
        let text = format!(
            "spike pid {pid}\nlast event: {last}\n\nEnter / click / SIGUSR1 resume   q quit"
        );
        terminal.draw(|frame| {
            let paragraph = Paragraph::new(text).block(Block::bordered().title("spike"));
            frame.render_widget(paragraph, frame.area());
        })?;
        if RESUME.load(Ordering::SeqCst) {
            return Ok(Outcome::Resume(Trigger::Signal));
        }
        if !event::poll(Duration::from_millis(250))? {
            continue;
        }
        let event = event::read()?;
        last = format!("{event:?}");
        match event {
            Event::Key(key) => {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Enter => return Ok(Outcome::Resume(Trigger::Key)),
                    KeyCode::Char('q') => return Ok(Outcome::Quit),
                    KeyCode::Backspace
                    | KeyCode::Left
                    | KeyCode::Right
                    | KeyCode::Up
                    | KeyCode::Down
                    | KeyCode::Home
                    | KeyCode::End
                    | KeyCode::PageUp
                    | KeyCode::PageDown
                    | KeyCode::Tab
                    | KeyCode::BackTab
                    | KeyCode::Delete
                    | KeyCode::Insert
                    | KeyCode::F(_)
                    | KeyCode::Char(_)
                    | KeyCode::Null
                    | KeyCode::Esc
                    | KeyCode::CapsLock
                    | KeyCode::ScrollLock
                    | KeyCode::NumLock
                    | KeyCode::PrintScreen
                    | KeyCode::Pause
                    | KeyCode::Menu
                    | KeyCode::KeypadBegin
                    | KeyCode::Media(_)
                    | KeyCode::Modifier(_) => {}
                }
            }
            Event::Mouse(mouse) => match mouse.kind {
                MouseEventKind::Down(_) => return Ok(Outcome::Resume(Trigger::Mouse)),
                MouseEventKind::Up(_)
                | MouseEventKind::Drag(_)
                | MouseEventKind::Moved
                | MouseEventKind::ScrollDown
                | MouseEventKind::ScrollUp
                | MouseEventKind::ScrollLeft
                | MouseEventKind::ScrollRight => {}
            },
            Event::FocusGained | Event::FocusLost | Event::Paste(_) | Event::Resize(_, _) => {}
        }
    }
}
