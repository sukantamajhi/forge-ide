//! An integrated terminal: a shell on a pseudo-terminal, parsed into a screen grid.

use std::io::{Read, Write};
use std::num::NonZeroU16;
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::thread;

use anyhow::{Context, Result};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

pub struct Terminal {
    parser: vt100::Parser,
    writer: Box<dyn Write + Send>,
    output: Receiver<Vec<u8>>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    rows: u16,
    cols: u16,
    /// The most recent raw bytes from the shell, kept to explain a silent terminal.
    debug_tail: Vec<u8>,
    /// Keep the tab after its process exits so the output can still be read (a debugged program's terminal).
    pub keep_when_dead: bool,
}

/// A run of cells sharing one style. `None` colors mean the terminal default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub fg: Option<u32>,
    pub bg: Option<u32>,
    pub bold: bool,
    pub underline: bool,
}

impl Terminal {
    /// The user's shell: PowerShell on Windows, `$SHELL` (or `sh`) elsewhere.
    pub fn default_shell() -> (String, Vec<String>) {
        if cfg!(windows) {
            ("powershell.exe".to_owned(), vec!["-NoLogo".to_owned()])
        } else {
            (std::env::var("SHELL").unwrap_or_else(|_| "sh".to_owned()), Vec::new())
        }
    }

    pub fn spawn(program: &str, args: &[String], cwd: &Path, rows: u16, cols: u16) -> Result<Self> {
        let pair = native_pty_system()
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .context("opening a pseudo-terminal")?;
        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        let child = pair.slave.spawn_command(cmd).with_context(|| format!("starting {program}"))?;
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().context("reading from the terminal")?;
        let writer = pair.master.take_writer().context("writing to the terminal")?;
        let (tx, output) = mpsc::channel();
        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        Ok(Self { parser: vt100::Parser::new(nz(rows), nz(cols), 2000), writer, output, master: pair.master, child, rows, cols, debug_tail: Vec::new(), keep_when_dead: false })
    }

    /// Feed any pending shell output to the screen. True if anything arrived.
    pub fn pump(&mut self) -> bool {
        let mut changed = false;
        while let Ok(bytes) = self.output.try_recv() {
            // Windows ConPTY asks where the cursor is and holds its output until answered.
            if bytes.windows(4).any(|w| w == b"[6n") {
                let (row, col) = self.parser.screen().cursor_position();
                self.send(format!("[{};{}R", row + 1, col + 1).as_bytes());
            }
            self.parser.process(&bytes);
            self.debug_tail.extend_from_slice(&bytes);
            if self.debug_tail.len() > 400 {
                self.debug_tail.drain(..self.debug_tail.len() - 400);
            }
            changed = true;
        }
        changed
    }

    /// The last raw bytes received, for diagnostics.
    pub fn debug_tail(&self) -> String {
        String::from_utf8_lossy(&self.debug_tail).into_owned()
    }

    pub fn send(&mut self, bytes: &[u8]) {
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        if (rows, cols) == (self.rows, self.cols) || rows == 0 || cols == 0 {
            return;
        }
        let _ = self.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
        self.parser.screen_mut().set_size(nz(rows), nz(cols));
        (self.rows, self.cols) = (rows, cols);
    }

    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    pub fn size(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }

    /// `(row, col, visible)` of the cursor.
    pub fn cursor(&self) -> (u16, u16, bool) {
        let screen = self.parser.screen();
        let (row, col) = screen.cursor_position();
        (row, col, !screen.hide_cursor())
    }

    /// All text on the screen, rows joined by newlines (for tests and copy).
    pub fn contents(&self) -> String {
        self.parser.screen().contents()
    }

    /// One screen row as styled runs.
    pub fn row_runs(&self, row: u16) -> Vec<Run> {
        let screen = self.parser.screen();
        let mut runs: Vec<Run> = Vec::new();
        for col in 0..self.cols {
            let Some(cell) = screen.cell(row, col) else { continue };
            if cell.is_wide_continuation() {
                continue;
            }
            let (mut fg, mut bg) = (color(cell.fgcolor()), color(cell.bgcolor()));
            if cell.inverse() {
                std::mem::swap(&mut fg, &mut bg);
            }
            let text = if cell.has_contents() { cell.contents().to_owned() } else { " ".to_owned() };
            match runs.last_mut() {
                Some(last) if last.fg == fg && last.bg == bg && last.bold == cell.bold() && last.underline == cell.underline() => {
                    last.text.push_str(&text)
                }
                _ => runs.push(Run { text, fg, bg, bold: cell.bold(), underline: cell.underline() }),
            }
        }
        runs
    }
}

fn nz(n: u16) -> NonZeroU16 {
    NonZeroU16::new(n.max(1)).unwrap_or(NonZeroU16::MIN)
}

fn color(c: vt100::Color) -> Option<u32> {
    match c {
        vt100::Color::Default => None,
        vt100::Color::Idx(i) => Some(ansi_to_rgb(i)),
        vt100::Color::Rgb(r, g, b) => Some(((r as u32) << 16) | ((g as u32) << 8) | b as u32),
    }
}

/// xterm's 256-color palette, with VS Code's dark theme for the first sixteen.
pub fn ansi_to_rgb(index: u8) -> u32 {
    const BASE: [u32; 16] = [
        0x000000, 0xcd3131, 0x0dbc79, 0xe5e510, 0x2472c8, 0xbc3fbc, 0x11a8cd, 0xe5e5e5, 0x666666, 0xf14c4c, 0x23d18b, 0xf5f543,
        0x3b8eea, 0xd670d6, 0x29b8db, 0xffffff,
    ];
    match index {
        0..=15 => BASE[index as usize],
        16..=231 => {
            let i = index as u32 - 16;
            let level = |n: u32| if n == 0 { 0 } else { 55 + n * 40 };
            (level(i / 36) << 16) | (level(i / 6 % 6) << 8) | level(i % 6)
        }
        232..=255 => {
            let v = 8 + 10 * (index as u32 - 232);
            (v << 16) | (v << 8) | v
        }
    }
}

/// Bytes a terminal program expects for a key press, or `None` if the key is not one.
pub fn key_bytes(key: &str, ctrl: bool, alt: bool, shift: bool, key_char: Option<&str>) -> Option<Vec<u8>> {
    let modifier = 1 + usize::from(shift) + 2 * usize::from(alt) + 4 * usize::from(ctrl);
    let csi = |code: &str, final_byte: char| -> Vec<u8> {
        if modifier > 1 { format!("\x1b[1;{modifier}{final_byte}").into_bytes() } else { format!("\x1b[{code}{final_byte}").into_bytes() }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if modifier > 1 { format!("\x1b[{n};{modifier}~").into_bytes() } else { format!("\x1b[{n}~").into_bytes() }
    };
    Some(match key {
        "enter" => b"\r".to_vec(),
        "backspace" => vec![if ctrl { 0x08 } else { 0x7f }],
        "tab" if shift => b"\x1b[Z".to_vec(),
        "tab" => b"\t".to_vec(),
        "escape" => vec![0x1b],
        "up" => csi("", 'A'),
        "down" => csi("", 'B'),
        "right" => csi("", 'C'),
        "left" => csi("", 'D'),
        "home" => csi("", 'H'),
        "end" => csi("", 'F'),
        "delete" => tilde(3),
        "insert" => tilde(2),
        "pageup" => tilde(5),
        "pagedown" => tilde(6),
        "space" if ctrl => vec![0],
        "space" if alt => b" ".to_vec(),
        "space" => b" ".to_vec(),
        "f1" => b"\x1bOP".to_vec(),
        "f2" => b"\x1bOQ".to_vec(),
        "f3" => b"\x1bOR".to_vec(),
        "f4" => b"\x1bOS".to_vec(),
        "f5" => tilde(15),
        "f6" => tilde(17),
        "f7" => tilde(18),
        "f8" => tilde(19),
        "f9" => tilde(20),
        "f10" => tilde(21),
        "f11" => tilde(23),
        "f12" => tilde(24),
        k if ctrl && k.len() == 1 && k.as_bytes()[0].is_ascii_alphabetic() => vec![k.as_bytes()[0].to_ascii_lowercase() & 0x1f],
        _ => {
            let text = key_char.filter(|t| !t.is_empty() && !ctrl)?;
            let mut out = if alt { vec![0x1b] } else { Vec::new() };
            out.extend_from_slice(text.as_bytes());
            out
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn keys_map_to_the_sequences_shells_expect() {
        assert_eq!(key_bytes("enter", false, false, false, None).unwrap(), b"\r");
        assert_eq!(key_bytes("up", false, false, false, None).unwrap(), b"\x1b[A");
        assert_eq!(key_bytes("left", true, false, false, None).unwrap(), b"\x1b[1;5D");
        assert_eq!(key_bytes("delete", false, false, false, None).unwrap(), b"\x1b[3~");
        assert_eq!(key_bytes("c", true, false, false, Some("c")).unwrap(), vec![3], "Ctrl+C is ETX");
        assert_eq!(key_bytes("tab", false, false, true, None).unwrap(), b"\x1b[Z");
        assert_eq!(key_bytes("a", false, false, false, Some("a")).unwrap(), b"a");
        assert_eq!(key_bytes("a", false, true, false, Some("a")).unwrap(), b"\x1ba");
        assert_eq!(key_bytes("é", false, false, false, Some("é")).unwrap(), "é".as_bytes());
        assert!(key_bytes("shift", false, false, true, None).is_none());
        // Space has no typed character on Windows, so it must be mapped by name.
        assert_eq!(key_bytes("space", false, false, false, None).unwrap(), b" ");
        assert_eq!(key_bytes("space", true, false, false, None).unwrap(), vec![0]);
    }

    #[test]
    fn palette_covers_base_cube_and_greys() {
        assert_eq!(ansi_to_rgb(1), 0xcd3131);
        assert_eq!(ansi_to_rgb(16), 0x000000);
        assert_eq!(ansi_to_rgb(231), 0xffffff);
        assert_eq!(ansi_to_rgb(196), 0xff0000);
        assert_eq!(ansi_to_rgb(232), 0x080808);
        assert_eq!(ansi_to_rgb(255), 0xeeeeee);
    }

    /// Runs a real shell command on a pseudo-terminal and reads its output off the screen.
    #[test]
    fn a_real_shell_runs_on_the_pty_and_its_output_reaches_the_screen() {
        let (program, args) = if cfg!(windows) {
            ("cmd.exe", vec!["/c".to_owned(), "echo forge-terminal-ok".to_owned()])
        } else {
            ("sh", vec!["-c".to_owned(), "echo forge-terminal-ok".to_owned()])
        };
        let mut term = Terminal::spawn(program, &args, &std::env::temp_dir(), 24, 80).expect("spawn");
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline && !term.contents().contains("forge-terminal-ok") {
            term.pump();
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(term.contents().contains("forge-terminal-ok"), "screen was {:?}; raw tail {:?}", term.contents(), term.debug_tail());
        let found = (0..24).any(|r| term.row_runs(r).iter().any(|run| run.text.contains("forge-terminal-ok")));
        assert!(found, "row_runs exposes the same text");
    }

    #[test]
    fn colored_output_becomes_styled_runs() {
        let mut parser = vt100::Parser::new(nz(2), nz(20), 0);
        parser.process(b"\x1b[31mred\x1b[0m plain");
        let mut term_like = Vec::new();
        for col in 0..9u16 {
            let cell = parser.screen().cell(0, col).unwrap();
            term_like.push((cell.contents().to_owned(), color(cell.fgcolor())));
        }
        assert_eq!(term_like[0], ("r".to_owned(), Some(0xcd3131)));
        assert_eq!(term_like[4].1, None, "after the reset the default color returns");
    }
}
