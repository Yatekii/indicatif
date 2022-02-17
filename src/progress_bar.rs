use std::borrow::Cow;
use std::fmt;
use std::io;
use std::sync::MutexGuard;
use std::sync::{Arc, Mutex, Weak};
use std::thread;
use std::time::{Duration, Instant};

use crate::draw_target::ProgressDrawTarget;
use crate::state::{BarState, ProgressState, Status};
use crate::style::ProgressStyle;
use crate::{ProgressBarIter, ProgressIterator};

/// A progress bar or spinner
///
/// The progress bar is an [`Arc`] around its internal state. When the progress bar is cloned it
/// just increments the refcount (so the original and its clone share the same state).
#[derive(Clone)]
pub struct ProgressBar {
    state: Arc<Mutex<BarState>>,
}

impl fmt::Debug for ProgressBar {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProgressBar").finish()
    }
}

impl ProgressBar {
    /// Creates a new progress bar with a given length
    ///
    /// This progress bar by default draws directly to stderr, and refreshes a maximum of 15 times
    /// a second. To change the refresh rate, set the draw target to one with a different refresh
    /// rate.
    pub fn new(len: u64) -> ProgressBar {
        ProgressBar::with_draw_target(len, ProgressDrawTarget::stderr())
    }

    /// Creates a completely hidden progress bar
    ///
    /// This progress bar still responds to API changes but it does not have a length or render in
    /// any way.
    pub fn hidden() -> ProgressBar {
        ProgressBar::with_draw_target(!0, ProgressDrawTarget::hidden())
    }

    /// Creates a new progress bar with a given length and draw target
    pub fn with_draw_target(len: u64, draw_target: ProgressDrawTarget) -> ProgressBar {
        ProgressBar {
            state: Arc::new(Mutex::new(BarState {
                draw_target,
                state: ProgressState::new(len),
            })),
        }
    }

    /// A convenience builder-like function for a progress bar with a given style
    pub fn with_style(self, style: ProgressStyle) -> ProgressBar {
        self.state.lock().unwrap().state.style = style;
        self
    }

    /// A convenience builder-like function for a progress bar with a given prefix
    pub fn with_prefix(self, prefix: impl Into<Cow<'static, str>>) -> ProgressBar {
        self.state.lock().unwrap().state.prefix = prefix.into();
        self
    }

    /// A convenience builder-like function for a progress bar with a given message
    pub fn with_message(self, message: impl Into<Cow<'static, str>>) -> ProgressBar {
        self.state.lock().unwrap().state.message = message.into();
        self
    }

    /// A convenience builder-like function for a progress bar with a given position
    pub fn with_position(self, pos: u64) -> ProgressBar {
        self.state.lock().unwrap().state.pos = pos;
        self
    }

    /// A convenience builder-like function for a progress bar with a given elapsed time
    pub fn with_elapsed(self, elapsed: Duration) -> ProgressBar {
        self.state.lock().unwrap().state.started = Instant::now() - elapsed;
        self
    }

    /// Creates a new spinner
    ///
    /// This spinner by default draws directly to stderr. This adds the default spinner style to it.
    pub fn new_spinner() -> ProgressBar {
        let rv = ProgressBar::new(!0);
        rv.set_style(ProgressStyle::default_spinner());
        rv
    }

    /// Overrides the stored style
    ///
    /// This does not redraw the bar. Call [`ProgressBar::tick()`] to force it.
    pub fn set_style(&self, style: ProgressStyle) {
        self.state.lock().unwrap().state.style = style;
    }

    /// Spawns a background thread to tick the progress bar
    ///
    /// When this is enabled a background thread will regularly tick the progress bar in the given
    /// interval (in milliseconds). This is useful to advance progress bars that are very slow by
    /// themselves.
    ///
    /// When steady ticks are enabled, calling [`ProgressBar::tick()`] on a progress bar does not
    /// have any effect.
    pub fn enable_steady_tick(&self, ms: u64) {
        let mut state = self.state.lock().unwrap();
        state.state.steady_tick = ms;
        if state.state.tick_thread.is_some() {
            return;
        }

        // Using a weak pointer is required to prevent a potential deadlock. See issue #133
        let state_arc = Arc::downgrade(&self.state);
        state.state.tick_thread = Some(thread::spawn(move || Self::steady_tick(state_arc, ms)));

        ::std::mem::drop(state);
        // use the side effect of tick to force the bar to tick.
        self.tick();
    }

    fn steady_tick(state_arc: Weak<Mutex<BarState>>, mut ms: u64) {
        loop {
            thread::sleep(Duration::from_millis(ms));
            if let Some(state_arc) = state_arc.upgrade() {
                let mut state = state_arc.lock().unwrap();
                if state.state.is_finished() || state.state.steady_tick == 0 {
                    state.state.steady_tick = 0;
                    state.state.tick_thread = None;
                    break;
                }
                if state.state.tick != 0 {
                    state.state.tick = state.state.tick.saturating_add(1);
                }
                ms = state.state.steady_tick;

                state.draw(false, Instant::now()).ok();
            } else {
                break;
            }
        }
    }

    /// Undoes [`ProgressBar::enable_steady_tick()`]
    pub fn disable_steady_tick(&self) {
        self.enable_steady_tick(0);
    }

    /// Manually ticks the spinner or progress bar
    ///
    /// This automatically happens on any other change to a progress bar.
    pub fn tick(&self) {
        self.update_and_draw(Instant::now(), |state| {
            if state.steady_tick == 0 || state.tick == 0 {
                state.tick = state.tick.saturating_add(1);
            }
        });
    }

    /// Advances the position of the progress bar by `delta`
    pub fn inc(&self, delta: u64) {
        self.update_and_draw(Instant::now(), |state| {
            state.pos = state.pos.saturating_add(delta);
            if state.steady_tick == 0 || state.tick == 0 {
                state.tick = state.tick.saturating_add(1);
            }
        })
    }

    /// A quick convenience check if the progress bar is hidden
    pub fn is_hidden(&self) -> bool {
        self.state.lock().unwrap().draw_target.is_hidden()
    }

    /// Indicates that the progress bar finished
    pub fn is_finished(&self) -> bool {
        self.state.lock().unwrap().state.is_finished()
    }

    /// Print a log line above the progress bar
    ///
    /// If the progress bar is hidden (e.g. when standard output is not a terminal), `println()`
    /// will not do anything. If you want to write to the standard output in such cases as well, use
    /// [`suspend`] instead.
    ///
    /// If the progress bar was added to a [`MultiProgress`], the log line will be
    /// printed above all other progress bars.
    ///
    /// [`suspend`]: ProgressBar::suspend
    /// [`MultiProgress`]: crate::MultiProgress
    pub fn println<I: AsRef<str>>(&self, msg: I) {
        let state = &mut *self.state.lock().unwrap();
        let draw_lines = state.state.should_render() && !state.draw_target.is_hidden();
        let (draw_target, state) = (&mut state.draw_target, &state.state);
        let width = draw_target.width();

        let mut drawable = match draw_target.drawable(true, Instant::now()) {
            Some(drawable) => drawable,
            None => return,
        };

        let mut draw_state = drawable.state();
        draw_state.move_cursor = false;
        draw_state.alignment = Default::default();

        draw_state
            .lines
            .extend(msg.as_ref().lines().map(Into::into));
        draw_state.orphan_lines = draw_state.lines.len();
        if draw_lines {
            state
                .style
                .format_state(state, &mut draw_state.lines, width);
        }

        drop(draw_state);
        let _ = drawable.draw();
    }

    /// Sets the position of the progress bar
    pub fn set_position(&self, pos: u64) {
        self.update_and_draw(Instant::now(), |state| {
            state.pos = pos;
            if state.steady_tick == 0 || state.tick == 0 {
                state.tick = state.tick.saturating_add(1);
            }
        })
    }

    /// Sets the length of the progress bar
    pub fn set_length(&self, len: u64) {
        self.update_and_draw(Instant::now(), |state| {
            state.len = len;
        })
    }

    /// Increase the length of the progress bar
    pub fn inc_length(&self, delta: u64) {
        self.update_and_draw(Instant::now(), |state| {
            state.len = state.len.saturating_add(delta);
        })
    }

    /// Sets the current prefix of the progress bar
    ///
    /// For the prefix to be visible, the `{prefix}` placeholder must be present in the template
    /// (see [`ProgressStyle`]).
    pub fn set_prefix(&self, prefix: impl Into<Cow<'static, str>>) {
        let prefix = prefix.into();
        self.update_and_draw(Instant::now(), |state| {
            state.prefix = prefix;
            if state.steady_tick == 0 || state.tick == 0 {
                state.tick = state.tick.saturating_add(1);
            }
        })
    }

    /// Sets the current message of the progress bar
    ///
    /// For the message to be visible, the `{msg}` placeholder must be present in the template (see
    /// [`ProgressStyle`]).
    pub fn set_message(&self, msg: impl Into<Cow<'static, str>>) {
        let msg = msg.into();
        self.update_and_draw(Instant::now(), |state| {
            state.message = msg;
            if state.steady_tick == 0 || state.tick == 0 {
                state.tick = state.tick.saturating_add(1);
            }
        })
    }

    /// Creates a new weak reference to this `ProgressBar`
    pub fn downgrade(&self) -> WeakProgressBar {
        WeakProgressBar {
            state: Arc::downgrade(&self.state),
        }
    }

    /// Resets the ETA calculation
    ///
    /// This can be useful if the progress bars made a large jump or was paused for a prolonged
    /// time.
    pub fn reset_eta(&self) {
        self.update_and_draw(Instant::now(), |state| {
            state.est.reset(state.pos);
        });
    }

    /// Resets elapsed time
    pub fn reset_elapsed(&self) {
        let now = Instant::now();
        self.update_and_draw(now, |state| {
            state.started = now;
        });
    }

    /// Resets all of the progress bar state
    pub fn reset(&self) {
        self.reset_eta();
        self.reset_elapsed();
        self.update_and_draw(Instant::now(), |state| {
            state.pos = 0;
            state.status = Status::InProgress;
        });
    }

    /// Finishes the progress bar and leaves the current message
    pub fn finish(&self) {
        self.state.lock().unwrap().finish(Instant::now());
    }

    /// Finishes the progress bar at current position and leaves the current message
    pub fn finish_at_current_pos(&self) {
        self.state
            .lock()
            .unwrap()
            .finish_at_current_pos(Instant::now());
    }

    /// Finishes the progress bar and sets a message
    ///
    /// For the message to be visible, the `{msg}` placeholder must be present in the template (see
    /// [`ProgressStyle`]).
    pub fn finish_with_message(&self, msg: impl Into<Cow<'static, str>>) {
        self.state
            .lock()
            .unwrap()
            .finish_with_message(msg, Instant::now());
    }

    /// Finishes the progress bar and completely clears it
    pub fn finish_and_clear(&self) {
        self.state.lock().unwrap().finish_and_clear(Instant::now());
    }

    /// Finishes the progress bar and leaves the current message and progress
    pub fn abandon(&self) {
        self.state.lock().unwrap().abandon(Instant::now());
    }

    /// Finishes the progress bar and sets a message, and leaves the current progress
    ///
    /// For the message to be visible, the `{msg}` placeholder must be present in the template (see
    /// [`ProgressStyle`]).
    pub fn abandon_with_message(&self, msg: impl Into<Cow<'static, str>>) {
        self.state
            .lock()
            .unwrap()
            .abandon_with_message(msg, Instant::now());
    }

    /// Finishes the progress bar using the behavior stored in the [`ProgressStyle`]
    ///
    /// See [`ProgressStyle::on_finish()`].
    pub fn finish_using_style(&self) {
        self.state
            .lock()
            .unwrap()
            .finish_using_style(Instant::now());
    }

    /// Sets a different draw target for the progress bar
    ///
    /// This can be used to draw the progress bar to stderr (this is the default):
    ///
    /// ```rust,no_run
    /// # use indicatif::{ProgressBar, ProgressDrawTarget};
    /// let pb = ProgressBar::new(100);
    /// pb.set_draw_target(ProgressDrawTarget::stderr());
    /// ```
    ///
    /// **Note:** Calling this method on a [`ProgressBar`] linked with a [`MultiProgress`] (after
    /// running [`MultiProgress::add`]) will unlink this progress bar. If you don't want this
    /// behavior, call [`MultiProgress::set_draw_target`] instead.
    ///
    /// [`MultiProgress`]: crate::MultiProgress
    /// [`MultiProgress::add`]: crate::MultiProgress::add
    /// [`MultiProgress::set_draw_target`]: crate::MultiProgress::set_draw_target
    pub fn set_draw_target(&self, target: ProgressDrawTarget) {
        let mut state = self.state.lock().unwrap();
        state.draw_target.disconnect(Instant::now());
        state.draw_target = target;
    }

    /// Hide the progress bar temporarily, execute `f`, then redraw the progress bar
    ///
    /// Useful for external code that writes to the standard output.
    ///
    /// **Note:** The internal lock is held while `f` is executed. Other threads trying to print
    /// anything on the progress bar will be blocked until `f` finishes.
    /// Therefore, it is recommended to avoid long-running operations in `f`.
    ///
    /// ```rust,no_run
    /// # use indicatif::ProgressBar;
    /// let mut pb = ProgressBar::new(3);
    /// pb.suspend(|| {
    ///     println!("Log message");
    /// })
    /// ```
    pub fn suspend<F: FnOnce() -> R, R>(&self, f: F) -> R {
        let mut state = self.state.lock().unwrap();
        let now = Instant::now();
        if let Some(drawable) = state.draw_target.drawable(true, now) {
            let _ = drawable.clear();
        }

        let ret = f();
        let _ = state.draw(true, now);
        ret
    }

    /// Wraps an [`Iterator`] with the progress bar
    ///
    /// ```rust,no_run
    /// # use indicatif::ProgressBar;
    /// let v = vec![1, 2, 3];
    /// let pb = ProgressBar::new(3);
    /// for item in pb.wrap_iter(v.iter()) {
    ///     // ...
    /// }
    /// ```
    pub fn wrap_iter<It: Iterator>(&self, it: It) -> ProgressBarIter<It> {
        it.progress_with(self.clone())
    }

    /// Wraps an [`io::Read`] with the progress bar
    ///
    /// ```rust,no_run
    /// # use std::fs::File;
    /// # use std::io;
    /// # use indicatif::ProgressBar;
    /// # fn test () -> io::Result<()> {
    /// let source = File::open("work.txt")?;
    /// let mut target = File::create("done.txt")?;
    /// let pb = ProgressBar::new(source.metadata()?.len());
    /// io::copy(&mut pb.wrap_read(source), &mut target);
    /// # Ok(())
    /// # }
    /// ```
    pub fn wrap_read<R: io::Read>(&self, read: R) -> ProgressBarIter<R> {
        ProgressBarIter {
            progress: self.clone(),
            it: read,
        }
    }

    /// Wraps an [`io::Write`] with the progress bar
    ///
    /// ```rust,no_run
    /// # use std::fs::File;
    /// # use std::io;
    /// # use indicatif::ProgressBar;
    /// # fn test () -> io::Result<()> {
    /// let mut source = File::open("work.txt")?;
    /// let target = File::create("done.txt")?;
    /// let pb = ProgressBar::new(source.metadata()?.len());
    /// io::copy(&mut source, &mut pb.wrap_write(target));
    /// # Ok(())
    /// # }
    /// ```
    pub fn wrap_write<W: io::Write>(&self, write: W) -> ProgressBarIter<W> {
        ProgressBarIter {
            progress: self.clone(),
            it: write,
        }
    }

    #[cfg(feature = "tokio")]
    /// Wraps an [`tokio::io::AsyncWrite`] with the progress bar
    ///
    /// ```rust,no_run
    /// # use tokio::fs::File;
    /// # use tokio::io;
    /// # use indicatif::ProgressBar;
    /// # async fn test() -> io::Result<()> {
    /// let mut source = File::open("work.txt").await?;
    /// let mut target = File::open("done.txt").await?;
    /// let pb = ProgressBar::new(source.metadata().await?.len());
    /// io::copy(&mut source, &mut pb.wrap_async_write(target)).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn wrap_async_write<W: tokio::io::AsyncWrite + Unpin>(
        &self,
        write: W,
    ) -> ProgressBarIter<W> {
        ProgressBarIter {
            progress: self.clone(),
            it: write,
        }
    }
    #[cfg(feature = "tokio")]
    /// Wraps an [`tokio::io::AsyncRead`] with the progress bar
    ///
    /// ```rust,no_run
    /// # use tokio::fs::File;
    /// # use tokio::io;
    /// # use indicatif::ProgressBar;
    /// # async fn test() -> io::Result<()> {
    /// let mut source = File::open("work.txt").await?;
    /// let mut target = File::open("done.txt").await?;
    /// let pb = ProgressBar::new(source.metadata().await?.len());
    /// io::copy(&mut pb.wrap_async_read(source), &mut target).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub fn wrap_async_read<W: tokio::io::AsyncRead + Unpin>(&self, write: W) -> ProgressBarIter<W> {
        ProgressBarIter {
            progress: self.clone(),
            it: write,
        }
    }

    fn update_and_draw<F: FnOnce(&mut ProgressState)>(&self, now: Instant, f: F) {
        // Delegate to the wrapped state.
        let mut state = self.state.lock().unwrap();
        state.update_and_draw(now, f);
    }

    /// Returns the current position
    pub fn position(&self) -> u64 {
        self.state.lock().unwrap().state.pos
    }

    /// Returns the current length
    pub fn length(&self) -> u64 {
        self.state.lock().unwrap().state.len
    }

    /// Returns the current ETA
    pub fn eta(&self) -> Duration {
        self.state.lock().unwrap().state.eta()
    }

    /// Returns the current rate of progress
    pub fn per_sec(&self) -> f64 {
        self.state.lock().unwrap().state.per_sec()
    }

    /// Returns the current expected duration
    pub fn duration(&self) -> Duration {
        self.state.lock().unwrap().state.duration()
    }

    /// Returns the current elapsed time
    pub fn elapsed(&self) -> Duration {
        self.state.lock().unwrap().state.started.elapsed()
    }

    /// Index in the `MultiState`
    pub(crate) fn index(&self) -> Option<usize> {
        self.state().draw_target.remote().map(|(_, idx)| idx)
    }

    pub(crate) fn state(&self) -> MutexGuard<'_, BarState> {
        self.state.lock().unwrap()
    }
}

/// A weak reference to a `ProgressBar`.
///
/// Useful for creating custom steady tick implementations
#[derive(Clone, Default)]
pub struct WeakProgressBar {
    state: Weak<Mutex<BarState>>,
}

impl WeakProgressBar {
    /// Create a new `WeakProgressBar` that returns `None` when [`upgrade`] is called.
    ///
    /// [`upgrade`]: WeakProgressBar::upgrade
    pub fn new() -> WeakProgressBar {
        Default::default()
    }

    /// Attempts to upgrade the Weak pointer to a [`ProgressBar`], delaying dropping of the inner
    /// value if successful. Returns `None` if the inner value has since been dropped.
    ///
    /// [`ProgressBar`]: struct.ProgressBar.html
    pub fn upgrade(&self) -> Option<ProgressBar> {
        self.state.upgrade().map(|state| ProgressBar { state })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::float_cmp)]
    #[test]
    fn test_pbar_zero() {
        let pb = ProgressBar::new(0);
        assert_eq!(pb.state.lock().unwrap().state.fraction(), 1.0);
    }

    #[allow(clippy::float_cmp)]
    #[test]
    fn test_pbar_maxu64() {
        let pb = ProgressBar::new(!0);
        assert_eq!(pb.state.lock().unwrap().state.fraction(), 0.0);
    }

    #[test]
    fn test_pbar_overflow() {
        let pb = ProgressBar::new(1);
        pb.set_draw_target(ProgressDrawTarget::hidden());
        pb.inc(2);
        pb.finish();
    }

    #[test]
    fn test_get_position() {
        let pb = ProgressBar::new(1);
        pb.set_draw_target(ProgressDrawTarget::hidden());
        pb.inc(2);
        let pos = pb.position();
        assert_eq!(pos, 2);
    }

    #[test]
    fn test_weak_pb() {
        let pb = ProgressBar::new(0);
        let weak = pb.downgrade();
        assert!(weak.upgrade().is_some());
        ::std::mem::drop(pb);
        assert!(weak.upgrade().is_none());
    }

    #[test]
    fn it_can_wrap_a_reader() {
        let bytes = &b"I am an implementation of io::Read"[..];
        let pb = ProgressBar::new(bytes.len() as u64);
        let mut reader = pb.wrap_read(bytes);
        let mut writer = Vec::new();
        io::copy(&mut reader, &mut writer).unwrap();
        assert_eq!(writer, bytes);
    }

    #[test]
    fn it_can_wrap_a_writer() {
        let bytes = b"implementation of io::Read";
        let mut reader = &bytes[..];
        let pb = ProgressBar::new(bytes.len() as u64);
        let writer = Vec::new();
        let mut writer = pb.wrap_write(writer);
        io::copy(&mut reader, &mut writer).unwrap();
        assert_eq!(writer.it, bytes);
    }
}
