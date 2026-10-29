#![cfg(windows)]

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::{Command as StdCommand, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use terminal_lab::{
    Key, KeyEvent, Modifiers, ProcessCommand, Terminal, TerminalConfig, TerminalError,
    TerminalSize, TerminalSnapshot, UpdateReceiver,
};

const TEST_TIMEOUT: Duration = Duration::from_secs(20);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(10);

struct TestTerminal {
    terminal: Option<Terminal>,
    // Keep the coalesced notification receiver alive for the whole test.
    _updates: UpdateReceiver,
}

impl TestTerminal {
    fn spawn(shell: &OsString, size: TerminalSize) -> Self {
        let command = ProcessCommand::new(shell, manifest_dir()).args(["-NoLogo", "-NoProfile"]);
        let config = TerminalConfig::new(command)
            .with_size(size)
            .with_scrollback_lines(100);
        let (terminal, updates) = Terminal::spawn(config).expect("spawn PowerShell ConPTY");
        Self {
            terminal: Some(terminal),
            _updates: updates,
        }
    }

    fn terminal(&self) -> &Terminal {
        self.terminal.as_ref().expect("terminal still open")
    }

    fn send_line(&self, line: &str) {
        // Deliberately split input, including inside UTF-8 code points.
        for chunk in line.as_bytes().chunks(7) {
            self.send_when_queue_has_room(chunk);
        }
        self.send_when_queue_has_room(b"\r");
    }

    fn send_when_queue_has_room(&self, bytes: &[u8]) {
        let deadline = Instant::now() + TEST_TIMEOUT;
        loop {
            match self.terminal().send_input(bytes) {
                Ok(()) => return,
                Err(TerminalError::InputQueueFull) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(5));
                }
                Err(error) => panic!("send terminal input: {error}"),
            }
        }
    }

    fn close(mut self) -> Result<(), TerminalError> {
        self.terminal
            .take()
            .expect("terminal still open")
            .close_timeout(CLOSE_TIMEOUT)
    }
}

impl Drop for TestTerminal {
    fn drop(&mut self) {
        if let Some(terminal) = self.terminal.take() {
            // A failed assertion must not strand the test's PowerShell process.
            terminal.close_timeout(CLOSE_TIMEOUT).ok();
        }
    }
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn powershell() -> Option<OsString> {
    let system = std::env::var_os("SystemRoot")?;
    [PathBuf::from(system)
        .join("System32/WindowsPowerShell/v1.0/powershell.exe")
        .into_os_string()]
    .into_iter()
    .find(|shell| {
        StdCommand::new(shell)
            .args([
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "exit 0",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    })
}

fn visible_text(snapshot: &TerminalSnapshot) -> String {
    (0..snapshot.size.rows())
        .map(|row| snapshot.row_text(row).expect("valid row"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn wait_for(
    terminal: &TestTerminal,
    description: &str,
    mut predicate: impl FnMut(&TerminalSnapshot) -> bool,
) -> TerminalSnapshot {
    let deadline = Instant::now() + TEST_TIMEOUT;
    loop {
        let snapshot = terminal.terminal().snapshot();
        if predicate(&snapshot) {
            return snapshot;
        }
        if terminal.terminal().is_reader_closed() {
            panic!(
                "PowerShell closed while waiting for {description}; I/O error: {:?}\n{}",
                terminal.terminal().last_io_error(),
                visible_text(&snapshot)
            );
        }
        if Instant::now() >= deadline {
            panic!(
                "timed out waiting for {description}; I/O error: {:?}\n{}",
                terminal.terminal().last_io_error(),
                visible_text(&snapshot)
            );
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_text(terminal: &TestTerminal, expected: &str) {
    wait_for(terminal, expected, |snapshot| {
        visible_text(snapshot).contains(expected)
    });
}

fn occurrence_count(haystack: &str, needle: &str) -> usize {
    haystack.match_indices(needle).count()
}

#[test]
fn powershell_output_utf8_cwd_and_resize() {
    let Some(shell) = powershell() else {
        eprintln!("SKIP: neither pwsh nor powershell is executable");
        return;
    };
    let initial_size = TerminalSize::new(27, 91).unwrap();
    let terminal = TestTerminal::spawn(&shell, initial_size);

    terminal.send_line("Write-Output ('PTY_' + 'READY')");
    wait_for_text(&terminal, "PTY_READY");

    let cwd = manifest_dir();
    terminal.send_line("Write-Output ('CWD=' + (Get-Location).Path)");
    wait_for(&terminal, "working directory", |snapshot| {
        let joined: String = visible_text(snapshot).lines().map(str::trim_end).collect();
        joined.contains(&format!("CWD={}", cwd.display()))
    });

    // ASCII-only input constructs output that cannot be confused with its echo.
    terminal.send_line(
        "[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); \
         Write-Output ('UTF8=' + [char]0x0416 + [char]0xD83E + [char]0xDD80)",
    );
    wait_for_text(&terminal, "UTF8=Ж🦀");

    terminal.send_line("Write-Output 'Ввод🦀'");
    wait_for(&terminal, "UTF-8 input echo and result", |snapshot| {
        occurrence_count(&visible_text(snapshot), "Ввод🦀") >= 2
    });

    terminal.send_line(
        "Write-Output ('SIZE=' + [string]$Host.UI.RawUI.WindowSize.Width + \
         'x' + [string]$Host.UI.RawUI.WindowSize.Height)",
    );
    wait_for_text(&terminal, "SIZE=91x27");

    let resized = TerminalSize::new(19, 73).unwrap();
    terminal.terminal().resize(resized).expect("resize ConPTY");
    assert_eq!(terminal.terminal().snapshot().size, resized);
    terminal.send_line(
        "Write-Output ('SIZE=' + [string]$Host.UI.RawUI.WindowSize.Width + \
         'x' + [string]$Host.UI.RawUI.WindowSize.Height)",
    );
    wait_for_text(&terminal, "SIZE=73x19");

    terminal.close().expect("close PowerShell");
}

#[test]
fn powershell_up_arrow_recalls_and_executes_history() {
    let Some(shell) = powershell() else {
        eprintln!("SKIP: neither pwsh nor powershell is executable");
        return;
    };
    let terminal = TestTerminal::spawn(&shell, TerminalSize::new(24, 100).unwrap());
    terminal.send_line("Write-Output ('HISTORY_' + 'OK')");
    wait_for_text(&terminal, "HISTORY_OK");

    terminal
        .terminal()
        .send_key(KeyEvent::new(Key::Up))
        .unwrap();
    terminal
        .terminal()
        .send_key(KeyEvent::new(Key::Enter))
        .unwrap();
    wait_for(
        &terminal,
        "second output from recalled history",
        |snapshot| occurrence_count(&visible_text(snapshot), "HISTORY_OK") >= 2,
    );

    terminal.close().expect("close PowerShell");
}

#[test]
fn powershell_ctrl_c_interrupts_a_running_command() {
    let Some(shell) = powershell() else {
        eprintln!("SKIP: neither pwsh nor powershell is executable");
        return;
    };
    let terminal = TestTerminal::spawn(&shell, TerminalSize::new(24, 100).unwrap());
    terminal.send_line("Write-Output ('SLEEP_' + 'BEGIN'); Start-Sleep -Seconds 30");
    let before_interrupt = wait_for(&terminal, "SLEEP_BEGIN", |snapshot| {
        visible_text(snapshot).contains("SLEEP_BEGIN")
    });
    let prompts_before = occurrence_count(&visible_text(&before_interrupt), "PS ");

    let ctrl = Modifiers {
        shift: false,
        alt: false,
        ctrl: true,
    };
    terminal
        .terminal()
        .send_key(KeyEvent::with_modifiers(Key::Char('c'), ctrl))
        .unwrap();
    wait_for(&terminal, "prompt after Ctrl+C", |snapshot| {
        occurrence_count(&visible_text(snapshot), "PS ") > prompts_before
    });
    terminal.send_line("Write-Output ('INTERRUPT_' + 'OK')");
    wait_for_text(&terminal, "INTERRUPT_OK");

    terminal.close().expect("close PowerShell");
}

#[test]
fn close_terminates_and_reaps_running_powershell() {
    let Some(shell) = powershell() else {
        eprintln!("SKIP: neither pwsh nor powershell is executable");
        return;
    };
    let terminal = TestTerminal::spawn(&shell, TerminalSize::new(24, 80).unwrap());
    terminal.send_line("Write-Output ('BUSY_' + 'BEGIN'); Start-Sleep -Seconds 30");
    wait_for_text(&terminal, "BUSY_BEGIN");
    assert!(terminal.terminal().process_id().is_some());
    terminal.close().expect("terminate and reap PowerShell");
}
