use console::Term;
use std::fmt::Debug;
use std::io;

/// A trait for minimal terminal-like behavior.
///
/// Anything that implements this trait can be used a draw target via [`ProgressDrawTarget::term_like`].
///
/// [`ProgressDrawTarget::term_like`]: crate::ProgressDrawTarget::term_like
pub trait TermLike: Debug + Send + Sync {
    /// Return the terminal width
    fn width(&self) -> usize;

    /// Move the cursor up by `n` lines
    fn move_cursor_up(&self, n: usize) -> io::Result<()>;
    /// Move the cursor down by `n` lines
    fn move_cursor_down(&self, n: usize) -> io::Result<()>;
    /// Move the cursor right by `n` lines
    fn move_cursor_right(&self, n: usize) -> io::Result<()>;
    /// Move the cursor left by `n` lines
    fn move_cursor_left(&self, n: usize) -> io::Result<()>;

    /// Write a string and add a newline.
    fn write_line(&self, s: &str) -> io::Result<()>;
    /// Write a string
    fn write_str(&self, s: &str) -> io::Result<()>;
    /// Clear the current line and reset the cursor to beginning of the line
    fn clear_line(&self) -> io::Result<()>;

    fn flush(&self) -> io::Result<()>;
}

impl TermLike for Term {
    fn width(&self) -> usize {
        self.size().1 as usize
    }

    fn move_cursor_up(&self, n: usize) -> io::Result<()> {
        self.move_cursor_up(n)
    }

    fn move_cursor_down(&self, n: usize) -> io::Result<()> {
        self.move_cursor_down(n)
    }

    fn move_cursor_right(&self, n: usize) -> io::Result<()> {
        self.move_cursor_right(n)
    }

    fn move_cursor_left(&self, n: usize) -> io::Result<()> {
        self.move_cursor_left(n)
    }

    fn write_line(&self, s: &str) -> io::Result<()> {
        self.write_line(s)
    }

    fn write_str(&self, s: &str) -> io::Result<()> {
        self.write_str(s)
    }

    fn clear_line(&self) -> io::Result<()> {
        self.clear_line()
    }

    fn flush(&self) -> io::Result<()> {
        self.flush()
    }
}
