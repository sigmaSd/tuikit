use std::{io, mem};

use super::cvt;
//use nix::libc::{c_int, c_ushort, ioctl, TIOCGWINSZ};

#[repr(C)]
struct TermSize {
    row: usize,
    col: usize,
    _x: usize,
    _y: usize,
}

/// Get the size of the terminal.
pub fn terminal_size() -> io::Result<(usize, usize)> {
    let (x, y) = crossterm::terminal::size().unwrap();
    Ok((x.into(), y.into()))
    // unsafe {
    //     let mut size: TermSize = mem::zeroed();
    //     cvt(ioctl(fd, TIOCGWINSZ.into(), &mut size as *mut _))?;
    //     Ok((size.col as usize, size.row as usize))
    // }
}
