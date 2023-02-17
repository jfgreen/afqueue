//! Selected FFI bindings to Kqueue.

use std::ffi::c_void;

//TODO: Doc all the kqueue stuff

pub type Kqueue = i32;

#[link(name = "c")]
extern "C" {

    pub fn kqueue() -> i32;

    pub fn kevent(
        kq: i32, //
        changelist: *const Kevent,
        nchanges: i32,
        eventlist: *mut Kevent,
        nevents: i32,
        timeout: *const Timespec,
    ) -> i32;

    pub fn read(descriptor: i32, buffer: *mut c_void, count: usize) -> isize;

    pub fn close(descriptor: i32) -> i32;
}

pub const EVFILT_READ: i16 = -1;
pub const EVFILT_TIMER: i16 = -7;
pub const EVFILT_USER: i16 = -10;
pub const EV_ADD: u16 = 0x1;
pub const EV_ENABLE: u16 = 0x4;
pub const EV_ONESHOT: u16 = 0x10;
pub const EV_CLEAR: u16 = 0x20;
pub const NOTE_TRIGGER: u32 = 0x01000000;
pub const NOTE_USECONDS: u32 = 0x00000002;

pub const STDIN_FILE_NUM: u64 = 0;

#[derive(Debug)]
#[repr(C)]
pub struct Timespec {
    tv_sec: i64,
    tv_nsec: i64,
}

#[derive(Debug, Clone, Copy, Default)]
#[repr(C)]
pub struct Kevent {
    /// Identifier for this event
    pub ident: u64,
    /// Filter for event
    pub filter: i16,
    /// Action flags for kqueue
    pub flags: u16,
    /// Filter flag value
    pub fflags: u32,
    /// Filter data value
    pub data: i64,
    /// Opaque user data identifier
    pub udata: u64,
}
