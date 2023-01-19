//! Selected FFI bindings to Termios

//TODO: Document termios stuff

use std::ffi::{c_char, c_ulong};

#[link(name = "c")]
extern "C" {
    pub fn tcgetattr(descriptor: i32, termios: *mut Termios) -> i32;
    pub fn tcsetattr(descriptor: i32, optional_actions: i32, termios: *const Termios) -> i32;
}

pub type TCFlag = c_ulong;
pub type CC = c_char;
pub type Speed = c_ulong;

/// Size of the `c_cc` control chars array.
pub const NCCS: usize = 20;

#[repr(C)]
#[derive(Copy, Clone, Default, Debug)]
pub struct Termios {
    /// input flags
    pub c_iflag: TCFlag,
    /// output flags
    pub c_oflag: TCFlag,
    /// control flags
    pub c_cflag: TCFlag,
    /// local flags
    pub c_lflag: TCFlag,
    /// control chars
    pub c_cc: [CC; NCCS],
    /// input speed
    pub c_ispeed: Speed,
    /// output speed
    pub c_ospeed: Speed,
}

/// Enable echoing
pub const ECHO: TCFlag = 0x00000008;

/// Canonicalize input lines (edit and submit input line by line)
pub const ICANON: TCFlag = 0x00000100;

/// Translate interupt, quit and suspend characters into corresponding signals
pub const ISIG: TCFlag = 0x00000080;

/// Enable output flow control
pub const IXON: TCFlag = 0x00000200;

/// Enable extended input procesing
pub const IEXTEN: TCFlag = 0x00000400;

/// Enable translation of carriage returns to newlines
pub const ICRNL: TCFlag = 0x00000100;

/// Enable output post processing
pub const OPOST: TCFlag = 0x00000001;

/// Enable sending SIGINT on break
pub const BRKINT: TCFlag = 0x00000002;

/// Drain output, flush input
pub const TCSAFLUSH: i32 = 2;
