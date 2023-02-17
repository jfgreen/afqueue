//! Selected FFI bindings to Termios

//TODO: Document termios stuff

use std::ffi::{c_int, c_uchar, c_ulong};

#[link(name = "c")]
extern "C" {
    pub fn tcgetattr(descriptor: c_int, termios: *mut Termios) -> c_int;
    pub fn tcsetattr(descriptor: c_int, optional_actions: c_int, termios: *const Termios) -> c_int;
}

pub type TCFlagT = c_ulong;
pub type Cct = c_uchar;
pub type SpeedT = c_ulong;

/// Size of the `c_cc` control chars array.
pub const NCCS: usize = 20;

#[repr(C)]
#[derive(Copy, Clone, Default, Debug)]
pub struct Termios {
    /// input flags
    pub c_iflag: TCFlagT,
    /// output flags
    pub c_oflag: TCFlagT,
    /// control flags
    pub c_cflag: TCFlagT,
    /// local flags
    pub c_lflag: TCFlagT,
    /// control chars
    pub c_cc: [Cct; NCCS],
    /// input speed
    pub c_ispeed: SpeedT,
    /// output speed
    pub c_ospeed: SpeedT,
}

/// Enable echoing
pub const ECHO: TCFlagT = 0x00000008;

/// Canonicalize input lines (edit and submit input line by line)
pub const ICANON: TCFlagT = 0x00000100;

/// Translate interupt, quit and suspend characters into corresponding signals
pub const ISIG: TCFlagT = 0x00000080;

/// Enable output flow control
pub const IXON: TCFlagT = 0x00000200;

/// Enable extended input procesing
pub const IEXTEN: TCFlagT = 0x00000400;

/// Enable translation of carriage returns to newlines
pub const ICRNL: TCFlagT = 0x00000100;

/// Enable output post processing
pub const OPOST: TCFlagT = 0x00000001;

/// Enable sending SIGINT on break
pub const BRKINT: TCFlagT = 0x00000002;

/// Drain output, flush input
pub const TCSAFLUSH: c_int = 2;
