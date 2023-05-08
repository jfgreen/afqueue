//! Selected FFI bindings to Ioctl

//TODO: Document ioctl stuff

use std::ffi::{c_int, c_ulong, c_ushort};

// NOTE: At some point POSIX might add terminal size information to termios
// For now, use ioctl instead

//TODO: Derive this magic number properly
pub const TIOCGWINSZ: c_ulong = 0x40087468;

#[link(name = "c")]
extern "C" {
    pub fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
}

#[derive(Debug)]
#[repr(C)]
pub struct WinSize {
    pub ws_row: c_ushort,
    pub ws_col: c_ushort,
    pub ws_xpixel: c_ushort,
    pub ws_ypixel: c_ushort,
}
