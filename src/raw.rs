//! Managing raw mode.
//!
//! Raw mode is a particular state a TTY can have. It signifies that:
//!
//! 1. No line buffering (the input is given byte-by-byte).
//! 2. The input is not written out, instead it has to be done manually by the programmer.
//! 3. The output is not canonicalized (for example, `\n` means "go one line down", not "line
//!    break").
//!
//! # Example
//!
//! ```rust,no_run
//! use tuikit::raw::IntoRawMode;
//! use std::io::{Write, stdout};
//!
//! fn main() {
//!     let mut stdout = stdout().into_raw_mode().unwrap();
//!
//!     write!(stdout, "Hey there.").unwrap();
//! }
//! ```

use std::io::{self, Write};
use std::ops;

//use nix::sys::termios::{cfmakeraw, tcgetattr, tcsetattr, SetArg, Termios};
//use nix::unistd::isatty;
//use nix::Error::Sys;
use std::fs;
use std::io::ErrorKind;
//use std::os::unix::io::{, RawFd};

// taken from termion
/// Get the TTY device.
///
/// This allows for getting stdio representing _only_ the TTY, and not other streams.
pub fn get_tty() -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
}

/// A terminal restorer, which keeps the previous state of the terminal, and restores it, when
/// dropped.
///
/// Restoring will entirely bring back the old TTY state.
pub struct RawTerminal<W: Write  > {
    prev_ios: (),
    output: W,
}

impl<W: Write  > Drop for RawTerminal<W> {
    fn drop(&mut self) {
        //let _ = tcsetattr(self.output.as_raw_fd(), SetArg::TCSANOW, &self.prev_ios);
    }
}

impl<W: Write  > ops::Deref for RawTerminal<W> {
    type Target = W;

    fn deref(&self) -> &W {
        &self.output
    }
}

impl<W: Write  > ops::DerefMut for RawTerminal<W> {
    fn deref_mut(&mut self) -> &mut W {
        &mut self.output
    }
}

impl<W: Write  > Write for RawTerminal<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.output.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.output.flush()
    }
}

// impl<W: Write> Write for RawTerminal<W> {
//     fn as_raw_fd(&self) -> RawFd {
//         return self.output.as_raw_fd();
//     }
// }

/// Types which can be converted into "raw mode".
///
/// # Why is this type defined on writers and not readers?
///
/// TTYs has their state controlled by the writer, not the reader. You use the writer to clear the
/// screen, move the cursor and so on, so naturally you use the writer to change the mode as well.
pub trait IntoRawMode: Write   + Sized {
    /// Switch to raw mode.
    ///
    /// Raw mode means that stdin won't be printed (it will instead have to be written manually by
    /// the program). Furthermore, the input isn't canonicalised or buffered (that is, you can
    /// read from stdin one byte of a time). The output is neither modified in any way.
    fn into_raw_mode(self) -> io::Result<RawTerminal<Self>>;
}

impl<W: Write  > IntoRawMode for W {
    // modified after https://github.com/kkawakam/rustyline/blob/master/src/tty/unix.rs#L668
    // refer: https://linux.die.net/man/3/termios
    fn into_raw_mode(self) -> io::Result<RawTerminal<W>> {
        crossterm::terminal::enable_raw_mode();
        Ok(RawTerminal {
            prev_ios:(),
            output: self,
        })
        // use nix::errno::Errno::ENOTTY;
        // use nix::sys::termios::OutputFlags;

        // let istty = isatty(self.as_raw_fd()).map_err(nix_err_to_io_err)?;
        // if !istty {
        //     Err(nix_err_to_io_err(nix::Error::from_errno(ENOTTY)))?
        // }

        // let prev_ios = tcgetattr(self.as_raw_fd()).map_err(nix_err_to_io_err)?;
        // let mut ios = prev_ios.clone();
        // // set raw mode
        // cfmakeraw(&mut ios);
        // // enable output processing (so that '\n' will issue carriage return)
        // ios.output_flags |= OutputFlags::OPOST;

        // tcsetattr(self.as_raw_fd(), SetArg::TCSANOW, &ios).map_err(nix_err_to_io_err)?;

        // Ok(RawTerminal {
        //     prev_ios,
        //     output: self,
        // })
    }
}

// fn nix_err_to_io_err(err: nix::Error) -> io::Error {
//     match err {
//         Sys(err_no) => io::Error::from(err_no),
//         _ => io::Error::new(ErrorKind::InvalidData, err),
//     }
// }

#[cfg(test)]
mod test {
    use super::*;
    use std::io::{stdout, Write};

    // #[test]
    // fn test_into_raw_mode() {
    //     let mut out = stdout().into_raw_mode().unwrap();
    //     out.write_all(b"this is a test, muahhahahah\r\n").unwrap();
    //     drop(out);
    // }
}
