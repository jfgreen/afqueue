use std::ffi::c_void;
use std::io;
use std::ptr;

use crate::ffi::kqueue::{self as kq, kevent, kqueue, Kevent, Kqueue};

const AUDIO_QUEUE_PLAYBACK_FINISHED: u64 = 40;
const UI_TIMER_TICK: u64 = 41;

const KEVENT_BUFFER_SIZE: usize = 10;
const INPUT_BUFFER_SIZE: usize = 10;

pub enum Event {
    PauseKeyPressed,
    ExitKeyPressed,
    AudioQueueStopped,
    UITick,
}

pub fn trigger_playback_finished_event(event_kqueue: Kqueue) {
    unsafe {
        let playback_finished_event = Kevent {
            ident: AUDIO_QUEUE_PLAYBACK_FINISHED,
            filter: kq::EVFILT_USER,
            flags: 0,
            fflags: kq::NOTE_TRIGGER,
            data: 0,
            udata: 0,
        };

        let changelist = [playback_finished_event];

        let result = kevent(
            event_kqueue,
            changelist.as_ptr(),
            changelist.len() as i32,
            ptr::null_mut(),
            0,
            ptr::null(),
        );

        if result < 0 {
            //TODO: Can we do better than this
            panic!(
                "Error triggering playback finished event: {}",
                io::Error::last_os_error()
            );
        }
    }
}

pub fn enable_playback_finished_event(kqueue: Kqueue) -> Result<(), io::Error> {
    unsafe {
        // Re enable the playback finished event
        let playback_finished_event = Kevent {
            ident: AUDIO_QUEUE_PLAYBACK_FINISHED,
            filter: kq::EVFILT_USER,
            flags: kq::EV_ENABLE,
            fflags: 0,
            data: 0,
            udata: 0,
        };

        let changelist = [playback_finished_event];

        // Register interest in both events
        let result = kevent(
            kqueue,
            changelist.as_ptr(),
            changelist.len() as i32,
            ptr::null_mut(),
            0,
            ptr::null(),
        );

        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

//TODO: Return error instead of panic
pub fn enable_ui_timer_event(kqueue: Kqueue, usec: i64) {
    unsafe {
        let ui_timer_event = Kevent {
            ident: UI_TIMER_TICK,
            filter: kq::EVFILT_TIMER,
            flags: kq::EV_ADD | kq::EV_ENABLE | kq::EV_ONESHOT,
            fflags: kq::NOTE_USECONDS,
            data: usec,
            udata: 0,
        };

        let changelist = [ui_timer_event];

        let result = kevent(
            kqueue,
            changelist.as_ptr(),
            changelist.len() as i32,
            ptr::null_mut(),
            0,
            ptr::null(),
        );

        if result < 0 {
            panic!("{}", io::Error::last_os_error());
        }
    }
}

//TODO: Refactor, think about abstractions thata might make it a little easier
// to follow
pub fn build_event_kqueue() -> Result<(Kqueue, EventReader), io::Error> {
    unsafe {
        let kqueue = kqueue();
        if kqueue < 0 {
            return Err(io::Error::last_os_error());
        }

        // TODO: See if EV_ENABLE is actually needed?

        // Describe the stdin events we are interested in
        let stdin_event = Kevent {
            ident: kq::STDIN_FILE_NUM,
            filter: kq::EVFILT_READ,
            flags: kq::EV_ADD | kq::EV_ENABLE,
            fflags: 0,
            data: 0,
            udata: 0,
        };

        // TODO: Maybe using a unique ident per file along with a EV_ONESHOT would be
        // easier?

        // Describe the playback finished events we are interested in
        // TODO: Increase confidence in using kqueue from one song to the next by using
        // udata to signal the audio queue thats stopped
        let playback_finished_event = Kevent {
            ident: AUDIO_QUEUE_PLAYBACK_FINISHED,
            filter: kq::EVFILT_USER,
            flags: kq::EV_ADD | kq::EV_DISPATCH | kq::EV_CLEAR,
            //flags:kq::EV_ADD | kq::EV_ONESHOT | kq::EV_ENABLE,
            fflags: 0,
            data: 0,
            udata: 0,
        };

        let changelist = [stdin_event, playback_finished_event];

        // Register interest in both events
        let result = kevent(
            kqueue,
            changelist.as_ptr(),
            changelist.len() as i32,
            ptr::null_mut(),
            0,
            ptr::null(),
        );

        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((kqueue, EventReader::new(kqueue)))
    }
}

pub fn close_kqueue(kqueue: Kqueue) -> Result<(), io::Error> {
    unsafe {
        let result = kq::close(kqueue);
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

pub struct EventReader {
    queue: KQueueReader,
    input: InputReader,
}

impl EventReader {
    fn new(event_kqueue: Kqueue) -> Self {
        EventReader {
            queue: KQueueReader::new(event_kqueue),
            input: InputReader::new(kq::STDIN_FILE_NUM as i32),
        }
    }

    pub fn next(&mut self) -> Event {
        // To get the next event we:
        // - Start by taking the next buffered char from stdin.
        // - If this char maps to a valid event then return, otherwise try again.
        // - If nothing buffered on std, instead perform a blocking read on the kqueue.
        // - If kqueue returns a user event, then return it.
        // - If the kqueue indicates that stdin has input to read, attempt to fill stdin
        //   and try again from the top.

        loop {
            if let Some(input_char) = self.input.read() {
                match input_char {
                    'q' => return Event::ExitKeyPressed,
                    'p' => return Event::PauseKeyPressed,
                    _ => continue,
                }
            }

            let queue_event = self.queue.read();
            let ident_filter = (queue_event.ident, queue_event.filter);

            match ident_filter {
                (kq::STDIN_FILE_NUM, kq::EVFILT_READ) => {
                    self.input.fill_buffer();
                    continue;
                }
                (AUDIO_QUEUE_PLAYBACK_FINISHED, kq::EVFILT_USER) => {
                    return Event::AudioQueueStopped
                }
                (UI_TIMER_TICK, kq::EVFILT_TIMER) => return Event::UITick,
                _ => continue,
            }
        }
    }
}

//TODO: Try and replace this with a std::io::Stdin buffered reader
struct InputReader {
    buffer: [u8; INPUT_BUFFER_SIZE],
    next: usize,
    filled: usize,
    file_descriptor: i32,
}

impl InputReader {
    fn new(file_descriptor: i32) -> Self {
        InputReader {
            buffer: [0; INPUT_BUFFER_SIZE],
            next: 0,
            filled: 0,
            file_descriptor,
        }
    }

    fn fill_buffer(&mut self) {
        unsafe {
            // NOTE: It's possible that the kqueue filter watching standard input might
            // spuriouly trigger. So this wont be guarenteed to read any bytes, even if
            // kqueue has reported there is input to read.

            let result = kq::read(
                self.file_descriptor,
                self.buffer.as_mut_ptr() as *mut c_void,
                self.buffer.len(),
            );

            if result < 0 {
                panic!("{}", io::Error::last_os_error());
            }

            self.next = 0;
            self.filled = result as usize;
        }
    }

    fn read(&mut self) -> Option<char> {
        if self.next == self.filled {
            return None;
        }
        let next_char = self.buffer[self.next] as char;
        self.next += 1;
        Some(next_char)
    }
}

//TODO: Would this be more ideomatic if we implemented the buf reader trait?
struct KQueueReader {
    buffer: [Kevent; KEVENT_BUFFER_SIZE],
    kqueue: Kqueue,
    next: usize,
    filled: usize,
}

impl KQueueReader {
    fn new(kqueue: Kqueue) -> Self {
        KQueueReader {
            kqueue,
            buffer: [Kevent::default(); KEVENT_BUFFER_SIZE],
            next: 0,
            filled: 0,
        }
    }

    fn read(&mut self) -> Kevent {
        unsafe {
            if self.next == self.filled {
                let result = kevent(
                    self.kqueue,
                    ptr::null(),
                    0,
                    self.buffer.as_mut_ptr(),
                    self.buffer.len() as i32,
                    ptr::null(),
                );

                if result < 0 {
                    panic!("{}", io::Error::last_os_error());
                }

                self.next = 0;
                self.filled = result as usize;
            }
            let item = self.buffer[self.next];
            self.next += 1;
            item
        }
    }
}
