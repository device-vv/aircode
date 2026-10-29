#![cfg(unix)]

use std::{
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};
use terminal_lab::{
    Key, KeyEvent, Modifiers, ProcessCommand, Terminal, TerminalConfig, TerminalSize,
    TerminalSnapshot, UpdateReceiver,
};

const TIMEOUT: Duration = Duration::from_secs(15);

struct TestTerminal {
    terminal: Option<Terminal>,
    _updates: UpdateReceiver,
}

impl TestTerminal {
    fn spawn() -> Self {
        let command = ProcessCommand::new("/bin/sh", manifest_dir());
        let config = TerminalConfig::new(command)
            .with_size(TerminalSize::new(24, 90).unwrap())
            .with_scrollback_lines(100);
        let (terminal, updates) = Terminal::spawn(config).expect("spawn system shell on a PTY");
        Self {
            terminal: Some(terminal),
            _updates: updates,
        }
    }

    fn terminal(&self) -> &Terminal {
        self.terminal.as_ref().expect("terminal is running")
    }

    fn send_line(&self, line: &str) {
        self.terminal().send_input(line.as_bytes()).unwrap();
        self.terminal().send_input(b"\n").unwrap();
    }

    fn close(mut self) {
        self.terminal
            .take()
            .unwrap()
            .close_timeout(TIMEOUT)
            .expect("close and reap system shell");
    }
}

impl Drop for TestTerminal {
    fn drop(&mut self) {
        if let Some(terminal) = self.terminal.take() {
            terminal.close_timeout(TIMEOUT).ok();
        }
    }
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn visible_text(snapshot: &TerminalSnapshot) -> String {
    (0..snapshot.size.rows())
        .map(|row| snapshot.row_text(row).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
}

fn wait_for(
    terminal: &TestTerminal,
    mut predicate: impl FnMut(&TerminalSnapshot) -> bool,
) -> TerminalSnapshot {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let snapshot = terminal.terminal().snapshot();
        if predicate(&snapshot) {
            return snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for shell output"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn system_shell_supports_unicode_resize_interrupt_history_and_cleanup() {
    let terminal = TestTerminal::spawn();
    terminal.send_line("printf 'PTY_READY\\n'");
    wait_for(&terminal, |snapshot| {
        visible_text(snapshot).contains("PTY_READY")
    });

    terminal.send_line("printf 'UTF8=Ж🦀\\n'");
    wait_for(&terminal, |snapshot| {
        visible_text(snapshot).contains("UTF8=Ж🦀")
    });

    terminal
        .terminal()
        .resize(TerminalSize::new(19, 73).unwrap())
        .expect("resize shell PTY");
    terminal.send_line("stty size");
    wait_for(&terminal, |snapshot| {
        visible_text(snapshot)
            .lines()
            .any(|line| line.trim() == "19 73")
    });

    terminal.send_line("printf 'SLEEP_BEGIN\\n'; sleep 30");
    wait_for(&terminal, |snapshot| {
        visible_text(snapshot).contains("SLEEP_BEGIN")
    });
    terminal
        .terminal()
        .send_key(KeyEvent::with_modifiers(
            Key::Char('c'),
            Modifiers {
                shift: false,
                alt: false,
                ctrl: true,
            },
        ))
        .unwrap();
    thread::sleep(Duration::from_millis(100));
    terminal.send_line("printf 'INTERRUPT_OK\\n'");
    wait_for(&terminal, |snapshot| {
        visible_text(snapshot).contains("INTERRUPT_OK")
    });

    terminal.send_line("i=0; while [ $i -lt 40 ]; do echo HISTORY_$i; i=$((i + 1)); done");
    let live = wait_for(&terminal, |snapshot| {
        visible_text(snapshot).contains("HISTORY_39")
    });
    assert!(live.scrollback > 0);
    let history = terminal.terminal().snapshot_at(live.scrollback);
    assert_eq!(history.scrollback_offset, live.scrollback);
    assert!(!history.cursor.visible);
    assert_ne!(history.row_text(0), live.row_text(0));

    terminal.close();
}
