//! stdin/stdout/stderr behind one injectable type, with TTY detection, color choice and
//! an optional pager. Tests use buffer-backed streams via `IoStreams::test` (`test-util`).

mod pager;

pub use pager::{NO_PAGER_ENV, is_truthy, resolve_pager_command};

use std::io::{self, BufRead, BufReader, Cursor, IsTerminal, Write};
use std::sync::{Arc, Mutex};

use self::pager::{PagerProcess, spawn_pager};

/// Effective color decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorChoice {
    /// Color because stdout is a terminal.
    Auto,
    /// Color forced by `CLICOLOR_FORCE=1`.
    Always,
    /// No color.
    Never,
}

impl ColorChoice {
    /// Pure resolution, highest priority first: `--no-color`, `NO_COLOR` (any value),
    /// `CLICOLOR_FORCE=1`, then whether stdout is a terminal.
    #[must_use]
    pub fn resolve(
        no_color_flag: bool,
        no_color_env: bool,
        clicolor_force: bool,
        stdout_tty: bool,
    ) -> Self {
        if no_color_flag || no_color_env {
            ColorChoice::Never
        } else if clicolor_force {
            ColorChoice::Always
        } else if stdout_tty {
            ColorChoice::Auto
        } else {
            ColorChoice::Never
        }
    }

    /// Whether ANSI color should be emitted.
    #[must_use]
    pub fn enabled(self) -> bool {
        matches!(self, ColorChoice::Auto | ColorChoice::Always)
    }
}

enum StdoutBackend {
    System,
    Pager(Box<dyn Write + Send>),
    #[cfg_attr(not(any(test, feature = "test-util")), allow(dead_code))]
    Buffer(Arc<Mutex<Vec<u8>>>),
}

enum StderrBackend {
    System,
    #[cfg_attr(not(any(test, feature = "test-util")), allow(dead_code))]
    Buffer(Arc<Mutex<Vec<u8>>>),
}

enum StdinBackend {
    System(BufReader<io::Stdin>),
    #[cfg_attr(not(any(test, feature = "test-util")), allow(dead_code))]
    Buffer(Cursor<Vec<u8>>),
}

impl StderrBackend {
    fn writer(&mut self) -> Box<dyn Write + '_> {
        match self {
            StderrBackend::System => Box::new(io::stderr().lock()),
            StderrBackend::Buffer(buf) => Box::new(BufferWriter {
                buf: Arc::clone(buf),
            }),
        }
    }
}

impl StdinBackend {
    fn reader(&mut self) -> &mut dyn BufRead {
        match self {
            StdinBackend::System(reader) => reader,
            StdinBackend::Buffer(cursor) => cursor,
        }
    }
}

/// The process streams as seen by the composition root.
pub struct IoStreams {
    stdin: StdinBackend,
    stdout: StdoutBackend,
    stderr: StderrBackend,
    stdin_tty: bool,
    stdout_tty: bool,
    stderr_tty: bool,
    color_enabled: bool,
    no_pager: bool,
    pager: Option<PagerProcess>,
}

impl IoStreams {
    /// The real process streams; color follows `--no-color`/`NO_COLOR`/`CLICOLOR_FORCE`/TTY.
    #[must_use]
    pub fn system(no_color: bool, no_pager: bool) -> Self {
        let stdout_tty = io::stdout().is_terminal();
        let color = ColorChoice::resolve(
            no_color,
            std::env::var_os("NO_COLOR").is_some(),
            std::env::var("CLICOLOR_FORCE").is_ok_and(|v| v == "1"),
            stdout_tty,
        );
        Self {
            stdin: StdinBackend::System(BufReader::new(io::stdin())),
            stdout: StdoutBackend::System,
            stderr: StderrBackend::System,
            stdin_tty: io::stdin().is_terminal(),
            stdout_tty,
            stderr_tty: io::stderr().is_terminal(),
            color_enabled: color.enabled(),
            no_pager,
            pager: None,
        }
    }

    /// Buffer-backed streams for tests: empty stdin, no TTY, no color, no pager.
    #[cfg(any(test, feature = "test-util"))]
    #[must_use]
    pub fn test() -> Self {
        Self::test_with_stdin("")
    }

    /// Like [`IoStreams::test`], with `input` available on stdin.
    #[cfg(any(test, feature = "test-util"))]
    #[must_use]
    pub fn test_with_stdin(input: &str) -> Self {
        Self {
            stdin: StdinBackend::Buffer(Cursor::new(input.as_bytes().to_vec())),
            stdout: StdoutBackend::Buffer(Arc::default()),
            stderr: StderrBackend::Buffer(Arc::default()),
            stdin_tty: false,
            stdout_tty: false,
            stderr_tty: false,
            color_enabled: false,
            no_pager: true,
            pager: None,
        }
    }

    /// Makes a test instance report stdin and stderr as terminals (or not); the streams
    /// stay buffers.
    #[cfg(any(test, feature = "test-util"))]
    #[must_use]
    pub fn with_tty(mut self, stdin: bool, stderr: bool) -> Self {
        self.stdin_tty = stdin;
        self.stderr_tty = stderr;
        self
    }

    /// A writer for the active stdout (terminal, pager pipe or test buffer).
    pub fn stdout(&mut self) -> Box<dyn Write + '_> {
        match &mut self.stdout {
            StdoutBackend::System => Box::new(io::stdout().lock()),
            StdoutBackend::Pager(pipe) => Box::new(PagerWriter {
                inner: pipe.as_mut(),
            }),
            StdoutBackend::Buffer(buf) => Box::new(BufferWriter {
                buf: Arc::clone(buf),
            }),
        }
    }

    /// A writer for stderr.
    pub fn stderr(&mut self) -> Box<dyn Write + '_> {
        self.stderr.writer()
    }

    /// The stdin reader; only read when a flag such as `--with-token` asks for it.
    pub fn stdin(&mut self) -> &mut dyn BufRead {
        self.stdin.reader()
    }

    /// [`IoStreams::stdin`] and [`IoStreams::stderr`] at once, for a handler that needs both.
    pub fn stdin_and_stderr(&mut self) -> (&mut dyn BufRead, Box<dyn Write + '_>) {
        (self.stdin.reader(), self.stderr.writer())
    }

    /// Whether stdin is a terminal.
    #[must_use]
    pub fn is_stdin_tty(&self) -> bool {
        self.stdin_tty
    }

    /// Whether stdout is a terminal.
    #[must_use]
    pub fn is_stdout_tty(&self) -> bool {
        self.stdout_tty
    }

    /// Whether stderr is a terminal.
    #[must_use]
    pub fn is_stderr_tty(&self) -> bool {
        self.stderr_tty
    }

    /// Whether ANSI color is enabled for stdout.
    #[must_use]
    pub fn color_enabled(&self) -> bool {
        self.color_enabled
    }

    /// Routes stdout through the pager when stdout is a terminal and paging is allowed.
    /// A second call while the pager runs is a no-op.
    pub fn start_pager(&mut self) {
        if self.pager.is_some()
            || self.no_pager
            || !self.stdout_tty
            || !matches!(self.stdout, StdoutBackend::System)
        {
            return;
        }
        let Some(mut process) = spawn_pager() else {
            return;
        };
        let Some(pipe) = process.take_stdin() else {
            return;
        };
        self.stdout = StdoutBackend::Pager(pipe);
        self.pager = Some(process);
    }

    /// Closes the pager pipe and waits for the pager to exit; idempotent.
    pub fn stop_pager(&mut self) {
        if matches!(self.stdout, StdoutBackend::Pager(_)) {
            // Dropping the pipe sends EOF to the pager.
            self.stdout = StdoutBackend::System;
        }
        if let Some(process) = self.pager.take() {
            process.wait();
        }
    }

    /// Captured stdout of a test instance.
    ///
    /// # Panics
    /// When called on system streams.
    #[cfg(any(test, feature = "test-util"))]
    #[must_use]
    pub fn stdout_as_string(&self) -> String {
        match &self.stdout {
            StdoutBackend::Buffer(buf) => buffer_string(buf),
            _ => panic!("stdout_as_string called on non-test IoStreams"),
        }
    }

    /// Captured stderr of a test instance.
    ///
    /// # Panics
    /// When called on system streams.
    #[cfg(any(test, feature = "test-util"))]
    #[must_use]
    pub fn stderr_as_string(&self) -> String {
        match &self.stderr {
            StderrBackend::Buffer(buf) => buffer_string(buf),
            StderrBackend::System => panic!("stderr_as_string called on non-test IoStreams"),
        }
    }
}

#[cfg(any(test, feature = "test-util"))]
fn buffer_string(buf: &Arc<Mutex<Vec<u8>>>) -> String {
    let guard = buf
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    String::from_utf8_lossy(&guard).into_owned()
}

impl Drop for IoStreams {
    fn drop(&mut self) {
        self.stop_pager();
    }
}

struct PagerWriter<'a> {
    inner: &'a mut (dyn Write + Send),
}

impl Write for PagerWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Appends to a shared buffer so the captured bytes outlive the writer.
struct BufferWriter {
    buf: Arc<Mutex<Vec<u8>>>,
}

impl Write for BufferWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.buf
            .lock()
            .map_err(|_| io::Error::other("buffer mutex poisoned"))?
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_resolution_order() {
        assert_eq!(
            ColorChoice::resolve(true, false, true, true),
            ColorChoice::Never
        );
        assert_eq!(
            ColorChoice::resolve(false, true, true, true),
            ColorChoice::Never
        );
        assert_eq!(
            ColorChoice::resolve(false, false, true, false),
            ColorChoice::Always
        );
        assert_eq!(
            ColorChoice::resolve(false, false, false, true),
            ColorChoice::Auto
        );
        assert_eq!(
            ColorChoice::resolve(false, false, false, false),
            ColorChoice::Never
        );
        assert!(!ColorChoice::Never.enabled());
    }

    #[test]
    fn test_streams_capture_output_and_serve_stdin() {
        let mut io = IoStreams::test_with_stdin("token-line\n");
        writeln!(io.stdout(), "out").unwrap();
        writeln!(io.stderr(), "err").unwrap();
        let mut line = String::new();
        io.stdin().read_line(&mut line).unwrap();
        assert_eq!(line, "token-line\n");
        assert_eq!(io.stdout_as_string(), "out\n");
        assert_eq!(io.stderr_as_string(), "err\n");
    }

    #[test]
    fn test_streams_are_not_terminals_unless_asked() {
        let io = IoStreams::test();
        assert!(!io.is_stdin_tty() && !io.is_stdout_tty() && !io.is_stderr_tty());
        let io = IoStreams::test().with_tty(true, false);
        assert!(io.is_stdin_tty() && !io.is_stderr_tty() && !io.is_stdout_tty());
    }

    #[test]
    fn stdin_and_stderr_are_usable_together() {
        let mut io = IoStreams::test_with_stdin("line\n");
        let (stdin, mut stderr) = io.stdin_and_stderr();
        let mut line = String::new();
        stdin.read_line(&mut line).unwrap();
        write!(stderr, "{line}").unwrap();
        drop(stderr);
        assert_eq!(io.stderr_as_string(), "line\n");
    }

    #[test]
    fn pager_never_starts_for_test_streams() {
        let mut io = IoStreams::test();
        io.start_pager();
        assert!(io.pager.is_none());
        io.stop_pager();
    }
}
