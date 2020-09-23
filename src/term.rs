//! Term is a thread-safe "terminal".
//!
//! It allows you to:
//! - Listen to key stroke events
//! - Output contents to the terminal
//!
//! ```no_run
//! use tuikit::prelude::*;
//!
//! let term = Term::new().unwrap();
//!
//! while let Ok(ev) = term.poll_event() {
//!     if let Event::Key(Key::Char('q')) = ev {
//!         break;
//!     }
//!
//!     term.print(0, 0, format!("got event: {:?}", ev).as_str());
//!     term.present();
//! }
//! ```
//!
//! Term is modeled after [termbox](https://github.com/nsf/termbox). The main idea is viewing
//! terminals as a table of fixed-size cells and input being a stream of structured messages

use std::cmp::{max, min};
use std::error::Error;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::attr::Attr;
use crate::canvas::Canvas;
use crate::cell::Cell;
use crate::draw::Draw;
use crate::event::Event;
//use crate::input::{KeyBoard, KeyboardHandler};
use crate::key::Key;
use crate::output::Command;
use crate::output::Output;
use crate::raw::{get_tty, IntoRawMode};
use crate::screen::Screen;
use crate::spinlock::SpinLock;
//use crate::sys::signal::{initialize_signals, notify_on_sigwinch, unregister_sigwinch};

pub type Result<T> = std::result::Result<T, Box<dyn Error>>;

const MIN_HEIGHT: usize = 1;
const WAIT_TIMEOUT: Duration = Duration::from_millis(300);
const POLLING_TIMEOUT: Duration = Duration::from_millis(10);

struct KeyBoard{}
impl KeyBoard {
    fn new_with_tty() -> Self {Self{}}
    fn get_interrupt_handler(&self) -> KeyboardHandler{KeyboardHandler{}}
    fn next_key(&self) -> Result<Key> {
        if let crossterm::event::Event::Key(kev) = crossterm::event::read().unwrap() {
            match kev {
                crossterm::event::KeyEvent{
                    code: crossterm::event::KeyCode::Char(c),
                    ..
                } => Ok(crate::key::Key::Char(c)),
                _ => todo!()
            }
        } else {
            todo!()
        }
    }
}

struct KeyboardHandler{}
impl KeyboardHandler {
    fn interrupt(&self) {}
}


#[derive(Debug)]
pub enum TermHeight {
    Fixed(usize),
    Percent(usize),
}

pub struct Term {
    components_to_stop: Arc<AtomicUsize>,
    keyboard_handler: SpinLock<Option<KeyboardHandler>>,
    resize_signal_id: Arc<AtomicUsize>,
    term_lock: SpinLock<TermLock>,
    event_rx: SpinLock<Receiver<Event>>,
    event_tx: Arc<SpinLock<Sender<Event>>>,
}

pub struct TermOptions {
    max_height: TermHeight,
    min_height: TermHeight,
    height: TermHeight,
    clear_on_exit: bool,
    mouse_enabled: bool,
}

impl Default for TermOptions {
    fn default() -> Self {
        Self {
            max_height: TermHeight::Percent(100),
            min_height: TermHeight::Fixed(3),
            height: TermHeight::Percent(100),
            clear_on_exit: true,
            mouse_enabled: false,
        }
    }
}

// Builder
impl TermOptions {
    pub fn max_height(mut self, max_height: TermHeight) -> Self {
        self.max_height = max_height;
        self
    }

    pub fn min_height(mut self, min_height: TermHeight) -> Self {
        self.min_height = min_height;
        self
    }
    pub fn height(mut self, height: TermHeight) -> Self {
        self.height = height;
        self
    }
    pub fn clear_on_exit(mut self, clear: bool) -> Self {
        self.clear_on_exit = clear;
        self
    }
    pub fn mouse_enabled(mut self, enabled: bool) -> Self {
        self.mouse_enabled = enabled;
        self
    }
}

impl Term {
    /// Create a Term with height specified.
    ///
    /// Internally if the calculated height would fill the whole screen, `Alternate Screen` will
    /// be enabled, otherwise only part of the screen will be used.
    ///
    /// If the preferred height is larger than the current screen, whole screen is used.
    ///
    /// ```no_run
    /// use tuikit::term::{Term, TermHeight};
    ///
    /// let term = Term::with_height(TermHeight::Percent(30)).unwrap(); // 30% of the terminal height
    /// let term = Term::with_height(TermHeight::Fixed(20)).unwrap(); // fixed 20 lines
    /// ```
    pub fn with_height(height: TermHeight) -> Result<Term> {
        Term::with_options(TermOptions::default().height(height))
    }

    /// Create a Term (with 100% height)
    ///
    /// ```no_run
    /// use tuikit::term::{Term, TermHeight};
    ///
    /// let term = Term::new().unwrap();
    /// let term = Term::with_height(TermHeight::Percent(100)).unwrap();
    /// ```
    pub fn new() -> Result<Term> {
        Term::with_options(TermOptions::default())
    }

    /// Create a Term with custom options
    ///
    /// ```no_run
    /// use tuikit::term::{Term, TermHeight, TermOptions};
    ///
    /// let term = Term::with_options(TermOptions::default().height(TermHeight::Percent(100)));
    /// ```
    pub fn with_options(options: TermOptions) -> Result<Term> {
//        initialize_signals();

        let (event_tx, event_rx) = channel();
        let ret = Term {
            components_to_stop: Arc::new(AtomicUsize::new(0)),
            keyboard_handler: SpinLock::new(None),
            resize_signal_id: Arc::new(AtomicUsize::new(0)),
            term_lock: SpinLock::new(TermLock::with_options(options)),
            event_tx: Arc::new(SpinLock::new(event_tx)),
            event_rx: SpinLock::new(event_rx),
        };
        ret.restart().map(|_| ret)
    }

    fn ensure_not_stopped(&self) -> Result<()> {
        if self.components_to_stop.load(Ordering::SeqCst) == 2 {
            Ok(())
        } else {
            Err("Terminal had been paused, should `restart` to use".into())
        }
    }

    fn get_cursor_pos(
        &self,
        keyboard: &mut KeyBoard,
        output: &mut Output,
    ) -> Result<(usize, usize)> {
        let pos = crossterm::cursor::position().unwrap();
        Ok((pos.0.into(), pos.1.into()))
        // output.ask_for_cpr();

        // if let Ok(key) = keyboard.next_key_timeout(WAIT_TIMEOUT) {
        //     if let Key::CursorPos(row, col) = key {
        //         return Ok((row as usize, col as usize));
        //     }
        // }

        // Ok((0, 0))
    }

    /// restart the terminal if it had been stopped
    pub fn restart(&self) -> Result<()> {
        let mut termlock = self.term_lock.lock();
        if self.components_to_stop.load(Ordering::SeqCst) == 2 {
            return Ok(());
        }

        let ttyout = get_tty()?.into_raw_mode()?;
        let mut output = Output::new(Box::new(ttyout))?;
        let mut keyboard = KeyBoard::new_with_tty();
        self.keyboard_handler
            .lock()
            .replace(keyboard.get_interrupt_handler());
        let cursor_pos = self.get_cursor_pos(&mut keyboard, &mut output)?;
        termlock.restart(output, cursor_pos)?;

        // start two listener
        self.start_key_listener(keyboard);
        self.start_size_change_listener();

        // wait for components to start
        while self.components_to_stop.load(Ordering::SeqCst) < 2 {
            debug!(
                "restart: components: {}",
                self.components_to_stop.load(Ordering::SeqCst)
            );
            thread::sleep(POLLING_TIMEOUT);
        }

        let event_tx = self.event_tx.lock();
        let _ = event_tx.send(Event::Restarted);

        Ok(())
    }

    /// Pause the Term
    ///
    /// This function will cause the Term to give away the control to the terminal(such as listening
    /// to the key strokes). After the Term was "paused", `poll_event` will block indefinitely and
    /// recover after the Term was `restart`ed.
    pub fn pause(&self) -> Result<()> {
        let mut termlock = self.term_lock.lock();

        if self.components_to_stop.load(Ordering::SeqCst) == 0 {
            return Ok(());
        }

        // wait for the components to stop
        // i.e. key_listener & size_change_listener
        self.keyboard_handler.lock().take().map(|h| h.interrupt());
//        unregister_sigwinch(self.resize_signal_id.load(Ordering::Relaxed)).map(|tx| tx.send(()));

        termlock.pause()?;

        // wait for the components to stop
        while self.components_to_stop.load(Ordering::SeqCst) > 0 {
            debug!(
                "pause: components: {}",
                self.components_to_stop.load(Ordering::SeqCst)
            );
            thread::sleep(POLLING_TIMEOUT);
        }

        Ok(())
    }

    fn start_key_listener(&self, mut keyboard: KeyBoard) {
        let event_tx_clone = self.event_tx.clone();
        let components_to_stop = self.components_to_stop.clone();
        thread::spawn(move || {
            components_to_stop.fetch_add(1, Ordering::SeqCst);
            debug!("key listener start");
            loop {
                let next_key = keyboard.next_key();
                trace!("next key: {:?}", next_key);
                if let Ok(key) = next_key {
                    let event_tx = event_tx_clone.lock();
                    let _ = event_tx.send(Event::Key(key));
                } else {
                    break;
                }
            }
            components_to_stop.fetch_sub(1, Ordering::SeqCst);
            debug!("key listener stop");
        });
    }

    fn start_size_change_listener(&self) {
        let event_tx_clone = self.event_tx.clone();
        let resize_signal_id = self.resize_signal_id.clone();
        let components_to_stop = self.components_to_stop.clone();

        // thread::spawn(move || {
        //     let (id, sigwinch_rx) = notify_on_sigwinch();
        //   resize_signal_id.store(id, Ordering::Relaxed);

        //     components_to_stop.fetch_add(1, Ordering::SeqCst);
        //     debug!("size change listener started");
        //     loop {
        //         if let Ok(_) = sigwinch_rx.recv() {
        //             let event_tx = event_tx_clone.lock();
        //             let _ = event_tx.send(Event::Resize {
        //                 width: 0,
        //                 height: 0,
        //             });
        //         } else {
        //             break;
        //         }
        //     }
        //     components_to_stop.fetch_sub(1, Ordering::SeqCst);
        //     debug!("size change listener stop");
        // });
    }

    fn filter_event(&self, event: Event) -> Event {
        match event {
            Event::Resize { .. } => {
                {
                    let mut termlock = self.term_lock.lock();
                    let _ = termlock.on_resize();
                }
                let (width, height) = self.term_size().unwrap_or((0, 0));
                Event::Resize { width, height }
            }
            Event::Key(Key::MousePress(button, row, col)) => {
                // adjust mouse event position
                let cursor_row = self.term_lock.lock().get_term_start_row() as u16;
                if row < cursor_row {
                    Event::__Nonexhaustive
                } else {
                    Event::Key(Key::MousePress(button, row - cursor_row, col))
                }
            }
            Event::Key(Key::MouseRelease(row, col)) => {
                // adjust mouse event position
                let cursor_row = self.term_lock.lock().get_term_start_row() as u16;
                if row < cursor_row {
                    Event::__Nonexhaustive
                } else {
                    Event::Key(Key::MouseRelease(row - cursor_row, col))
                }
            }
            Event::Key(Key::MouseHold(row, col)) => {
                // adjust mouse event position
                let cursor_row = self.term_lock.lock().get_term_start_row() as u16;
                if row < cursor_row {
                    Event::__Nonexhaustive
                } else {
                    Event::Key(Key::MouseHold(row - cursor_row, col))
                }
            }
            ev => ev,
        }
    }

    /// Wait an event up to `timeout` and return it
    pub fn peek_event(&self, timeout: Duration) -> Result<Event> {
        let event_rx = self.event_rx.lock();
        event_rx
            .recv_timeout(timeout)
            .map(|ev| self.filter_event(ev))
            .map_err(|_| "timeout".to_string().into())
    }

    /// Wait for an event indefinitely and return it
    pub fn poll_event(&self) -> Result<Event> {
        let event_rx = self.event_rx.lock();
        event_rx
            .recv()
            .map(|ev| self.filter_event(ev))
            .map_err(|err| err.to_string().into())
    }

    /// An interface to inject event to the terminal's event queue
    pub fn send_event(&self, event: Event) -> Result<()> {
        let event_tx = self.event_tx.lock();
        event_tx.send(event).map_err(|err| err.to_string().into())
    }

    /// Sync internal buffer with terminal
    pub fn present(&self) -> Result<()> {
        self.ensure_not_stopped()?;
        let mut termlock = self.term_lock.lock();
        termlock.present()
    }

    /// Return the printable size(width, height) of the term
    pub fn term_size(&self) -> Result<(usize, usize)> {
        self.ensure_not_stopped()?;
        let termlock = self.term_lock.lock();
        Ok(termlock.term_size()?)
    }

    /// Clear internal buffer
    pub fn clear(&self) -> Result<()> {
        self.ensure_not_stopped()?;
        let mut termlock = self.term_lock.lock();
        termlock.clear()
    }

    /// Change a cell of position `(row, col)` to `cell`
    pub fn put_cell(&self, row: usize, col: usize, cell: Cell) -> Result<usize> {
        self.ensure_not_stopped()?;
        let mut termlock = self.term_lock.lock();
        termlock.put_cell(row, col, cell)
    }

    /// Print `content` starting with position `(row, col)`
    pub fn print(&self, row: usize, col: usize, content: &str) -> Result<usize> {
        self.print_with_attr(row, col, content, Attr::default())
    }

    /// print `content` starting with position `(row, col)` with `attr`
    pub fn print_with_attr(
        &self,
        row: usize,
        col: usize,
        content: &str,
        attr: impl Into<Attr>,
    ) -> Result<usize> {
        self.ensure_not_stopped()?;
        let mut termlock = self.term_lock.lock();
        termlock.print_with_attr(row, col, content, attr)
    }

    /// Set cursor position to (row, col), and show the cursor
    pub fn set_cursor(&self, row: usize, col: usize) -> Result<()> {
        self.ensure_not_stopped()?;
        let mut termlock = self.term_lock.lock();
        termlock.set_cursor(row, col)
    }

    /// show/hide cursor, set `show` to `false` to hide the cursor
    pub fn show_cursor(&self, show: bool) -> Result<()> {
        self.ensure_not_stopped()?;
        let mut termlock = self.term_lock.lock();
        termlock.show_cursor(show)
    }

    /// Enable mouse support
    pub fn enable_mouse_support(&self) -> Result<()> {
        self.ensure_not_stopped()?;
        let mut termlock = self.term_lock.lock();
        termlock.enable_mouse_support()
    }

    /// Disable mouse support
    pub fn disable_mouse_support(&self) -> Result<()> {
        self.ensure_not_stopped()?;
        let mut termlock = self.term_lock.lock();
        termlock.disable_mouse_support()
    }

    /// Whether to clear the terminal upon exiting. Defaults to true.
    pub fn clear_on_exit(&self, clear: bool) -> Result<()> {
        self.ensure_not_stopped()?;
        let mut termlock = self.term_lock.lock();
        termlock.clear_on_exit(clear);
        Ok(())
    }

    pub fn draw(&self, draw: &dyn Draw) -> Result<()> {
        let mut canvas = TermCanvas { term: &self };
        draw.draw(&mut canvas)
    }
}

pub struct TermCanvas<'a> {
    term: &'a Term,
}

impl<'a> Canvas for TermCanvas<'a> {
    fn size(&self) -> Result<(usize, usize)> {
        self.term.term_size()
    }

    fn clear(&mut self) -> Result<()> {
        self.term.clear()
    }

    fn put_cell(&mut self, row: usize, col: usize, cell: Cell) -> Result<usize> {
        self.term.put_cell(row, col, cell)
    }

    fn print_with_attr(
        &mut self,
        row: usize,
        col: usize,
        content: &str,
        attr: Attr,
    ) -> Result<usize> {
        self.term.print_with_attr(row, col, content, attr)
    }

    fn set_cursor(&mut self, row: usize, col: usize) -> Result<()> {
        self.term.set_cursor(row, col)
    }

    fn show_cursor(&mut self, show: bool) -> Result<()> {
        self.term.show_cursor(show)
    }
}

struct TermLock {
    prefer_height: TermHeight,
    max_height: TermHeight,
    min_height: TermHeight,
    // keep bottom intact when resize?
    bottom_intact: bool,
    clear_on_exit: bool,
    mouse_enabled: bool,
    alternate_screen: bool,
    cursor_row: usize,
    screen_height: usize,
    screen_width: usize,
    screen: Screen,
    output: Option<Output>,
}

impl Default for TermLock {
    fn default() -> Self {
        Self {
            prefer_height: TermHeight::Percent(100),
            max_height: TermHeight::Percent(100),
            min_height: TermHeight::Fixed(3),
            bottom_intact: false,
            alternate_screen: false,
            cursor_row: 0,
            screen_height: 0,
            screen_width: 0,
            screen: Screen::new(0, 0),
            output: None,
            clear_on_exit: true,
            mouse_enabled: false,
        }
    }
}

impl TermLock {
    pub fn with_options(options: TermOptions) -> Self {
        let mut term = TermLock::default();
        term.prefer_height = options.height;
        term.max_height = options.max_height;
        term.min_height = options.min_height;
        term.clear_on_exit = options.clear_on_exit;
        term.mouse_enabled = options.mouse_enabled;
        term
    }

    /// Present the content to the terminal
    pub fn present(&mut self) -> Result<()> {
        let output = self.output.as_mut().ok_or("term had been stopped")?;
        let mut commands = self.screen.present();

        let cursor_row = self.cursor_row;
        // add cursor_row to all CursorGoto commands
        for cmd in commands.iter_mut() {
            if let Command::CursorGoto { row, col } = *cmd {
                *cmd = Command::CursorGoto {
                    row: row + cursor_row,
                    col,
                }
            }
        }

        for cmd in commands.into_iter() {
            output.execute(cmd);
        }
        output.flush();
        Ok(())
    }

    /// Resize the internal buffer to according to new terminal size
    pub fn on_resize(&mut self) -> Result<()> {
        let output = self.output.as_mut().ok_or("term had been stopped")?;
        let (screen_width, screen_height) = output
            .terminal_size()
            .expect("term:restart get terminal size failed");
        self.screen_height = screen_height;
        self.screen_width = screen_width;

        let width = screen_width;
        let height = Self::calc_preferred_height(
            &self.min_height,
            &self.max_height,
            &self.prefer_height,
            screen_height,
        );

        // update the cursor position
        if self.cursor_row + height >= screen_height {
            self.bottom_intact = true;
        }

        if self.bottom_intact {
            self.cursor_row = screen_height - height;
        }

        // clear the screen
        let _ = output.cursor_goto(self.cursor_row, 0);
        let _ = output.erase_down();

        // clear the screen buffer
        self.screen.resize(width, height);
        Ok(())
    }

    fn calc_height(height_spec: &TermHeight, actual_height: usize) -> usize {
        match *height_spec {
            TermHeight::Fixed(h) => h,
            TermHeight::Percent(p) => actual_height * min(p, 100) / 100,
        }
    }

    fn calc_preferred_height(
        min_height: &TermHeight,
        max_height: &TermHeight,
        prefer_height: &TermHeight,
        height: usize,
    ) -> usize {
        let max_height = Self::calc_height(max_height, height);
        let min_height = Self::calc_height(min_height, height);
        let prefer_height = Self::calc_height(prefer_height, height);

        // ensure the calculated height is in range (MIN_HEIGHT, height)
        let max_height = max(min(max_height, height), MIN_HEIGHT);
        let min_height = max(min(min_height, height), MIN_HEIGHT);
        max(min(prefer_height, max_height), min_height)
    }

    /// Pause the terminal
    pub fn pause(&mut self) -> Result<()> {
        self.disable_mouse()?;
        self.output.take().map(|mut output| {
            // clear drawn contents
            if self.alternate_screen {
                output.quit_alternate_screen();
            } else {
                output.cursor_goto(self.cursor_row, 0);
                output.show_cursor();
                output.erase_down();
            }
            output.flush();
        });
        Ok(())
    }

    /// ensure the screen had enough height
    /// If the prefer height is full screen, it will enter alternate screen
    /// otherwise it will ensure there are enough lines at the bottom
    fn ensure_height(&mut self, cursor_pos: (usize, usize)) -> Result<()> {
        let output = self.output.as_mut().ok_or("term had been stopped")?;

        // initialize

        let (screen_width, screen_height) = output
            .terminal_size()
            .expect("termlock:ensure_height get terminal size failed");
        let height_to_be = Self::calc_preferred_height(
            &self.min_height,
            &self.max_height,
            &self.prefer_height,
            screen_height,
        );

        self.alternate_screen = false;
        let (mut cursor_row, cursor_col) = cursor_pos;
        if height_to_be >= screen_height {
            // whole screen
            self.alternate_screen = true;
            self.bottom_intact = false;
            self.cursor_row = 0;
            output.enter_alternate_screen();
        } else {
            // only use part of the screen

            // go to a new line so that existing line won't be messed up
            if cursor_col > 0 {
                output.write("\n");
                cursor_row += 1;
            }

            if (cursor_row + height_to_be) <= screen_height {
                self.bottom_intact = false;
                self.cursor_row = cursor_row;
            } else {
                for _ in 0..(height_to_be - 1) {
                    output.write("\n");
                }
                self.bottom_intact = true;
                self.cursor_row = min(cursor_row, screen_height - height_to_be);
            }
        }

        output.cursor_goto(self.cursor_row, 0);
        output.flush();
        self.screen_height = screen_height;
        self.screen_width = screen_width;
        Ok(())
    }

    /// get the start row of the terminal
    pub fn get_term_start_row(&self) -> usize {
        self.cursor_row
    }

    /// restart the terminal
    pub fn restart(&mut self, output: Output, cursor_pos: (usize, usize)) -> Result<()> {
        // ensure the output area had enough height
        self.output.replace(output);
        self.ensure_height(cursor_pos)?;
        self.on_resize()?;
        if self.mouse_enabled {
            self.enable_mouse()?;
        }
        Ok(())
    }

    /// return the printable size(width, height) of the term
    pub fn term_size(&self) -> Result<(usize, usize)> {
        self.screen.size()
    }

    /// clear internal buffer
    pub fn clear(&mut self) -> Result<()> {
        self.screen.clear()
    }

    /// change a cell of position `(row, col)` to `cell`
    pub fn put_cell(&mut self, row: usize, col: usize, cell: Cell) -> Result<usize> {
        self.screen.put_cell(row, col, cell)
    }

    /// print `content` starting with position `(row, col)`
    pub fn print_with_attr(
        &mut self,
        row: usize,
        col: usize,
        content: &str,
        attr: impl Into<Attr>,
    ) -> Result<usize> {
        self.screen.print_with_attr(row, col, content, attr.into())
    }

    /// set cursor position to (row, col)
    pub fn set_cursor(&mut self, row: usize, col: usize) -> Result<()> {
        self.screen.set_cursor(row, col)
    }

    /// show/hide cursor, set `show` to `false` to hide the cursor
    pub fn show_cursor(&mut self, show: bool) -> Result<()> {
        self.screen.show_cursor(show)
    }

    /// Enable mouse support
    pub fn enable_mouse_support(&mut self) -> Result<()> {
        self.mouse_enabled = true;
        self.enable_mouse()
    }

    /// Disable mouse support
    pub fn disable_mouse_support(&mut self) -> Result<()> {
        self.mouse_enabled = false;
        self.disable_mouse()
    }

    pub fn clear_on_exit(&mut self, clear: bool) {
        self.clear_on_exit = clear;
    }

    /// Enable mouse (send ANSI codes to enable mouse)
    fn enable_mouse(&mut self) -> Result<()> {
        let output = self.output.as_mut().ok_or("term had been stopped")?;
        output.enable_mouse_support();
        Ok(())
    }

    /// Disable mouse (send ANSI codes to disable mouse)
    fn disable_mouse(&mut self) -> Result<()> {
        let output = self.output.as_mut().ok_or("term had been stopped")?;
        output.disable_mouse_support();
        Ok(())
    }
}

impl Drop for TermLock {
    fn drop(&mut self) {
        if self.clear_on_exit {
            let _ = self.pause();
        } else {
            self.output.take().map(|mut output| {
                output.cursor_goto(self.cursor_row + self.screen.height(), 0);
                if self.bottom_intact {
                    output.write("\n");
                }
                output.show_cursor();
                output.flush();
            });
        }
    }
}
