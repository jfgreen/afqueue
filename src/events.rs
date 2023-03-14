use std::ffi::c_void;
use std::fmt;
use std::io;
use std::ptr;

use crate::ffi::kqueue::{self as kq, kevent, kqueue, Kevent, Kqueue};

const AUDIO_QUEUE_PLAYBACK_FINISHED: u64 = 40;
const UI_TIMER_TICK: u64 = 41;

const KEVENT_BUFFER_SIZE: usize = 10;
const INPUT_BUFFER_SIZE: usize = 10;

#[derive(Debug)]
pub enum EventError {
    Create(io::Error),
    Add(io::Error),
    Trigger(io::Error),
    Enable(io::Error),
    Close(io::Error),
}

impl fmt::Display for EventError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            EventError::Create(err) => {
                write!(f, "IO error creating kqueue'{err}'")
            }
            EventError::Add(err) => {
                write!(f, "IO error adding event filter '{err}'")
            }
            EventError::Trigger(err) => {
                write!(f, "IO error triggering event '{err}'")
            }
            EventError::Enable(err) => {
                write!(f, "IO error enabling event '{err}'")
            }
            EventError::Close(err) => {
                write!(f, "IO error closing kqueue '{err}'")
            }
        }
    }
}

pub enum Event {
    NextTrackKeyPressed,
    PauseKeyPressed,
    ExitKeyPressed,
    AudioQueueStopped,
    UITick,
    TerminalResized,
}

#[derive(Clone)]
pub struct Sender {
    queue: Kqueue,
}

impl Sender {
    pub fn trigger_playback_finished_event(&mut self) -> Result<(), EventError> {
        //TODO: Extract function for writing to kqueue
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
                self.queue,
                changelist.as_ptr(),
                changelist.len() as i32,
                ptr::null_mut(),
                0,
                ptr::null(),
            );

            if result < 0 {
                let io_err = io::Error::last_os_error();
                return Err(EventError::Trigger(io_err));
            }
            Ok(())
        }
    }
}

pub struct Receiver {
    queue: Kqueue,
    queue_reader: KQueueReader,
    input_reader: InputReader,
}

impl Receiver {
    pub fn next(&mut self) -> Event {
        // To get the next event we:
        // - Start by taking the next buffered char from stdin.
        // - If this char maps to a valid event then return, otherwise try again.
        // - If nothing buffered on std, instead perform a blocking read on the kqueue.
        // - If kqueue returns a user event, then return it.
        // - If the kqueue indicates that stdin has input to read, attempt to fill stdin
        //   and try again from the top. TODO: Flow chart would be nice

        loop {
            if let Some(input_char) = self.input_reader.read() {
                match input_char {
                    'n' => return Event::NextTrackKeyPressed,
                    'q' => return Event::ExitKeyPressed,
                    'p' => return Event::PauseKeyPressed,
                    _ => continue,
                }
            }

            let queue_event = self.queue_reader.read();
            let ident_filter = (queue_event.ident, queue_event.filter);

            match ident_filter {
                (kq::STDIN_FILE_NUM, kq::EVFILT_READ) => {
                    self.input_reader.fill_buffer();
                    continue;
                }
                (AUDIO_QUEUE_PLAYBACK_FINISHED, kq::EVFILT_USER) => {
                    return Event::AudioQueueStopped
                }
                (UI_TIMER_TICK, kq::EVFILT_TIMER) => return Event::UITick,
                (kq::SIGWINCH, kq::EVFILT_SIGNAL) => return Event::TerminalResized,
                _ => continue,
            }
        }
    }

    pub fn enable_ui_timer_event(&mut self, usec: i64) -> Result<(), EventError> {
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
                self.queue,
                changelist.as_ptr(),
                changelist.len() as i32,
                ptr::null_mut(),
                0,
                ptr::null(),
            );

            if result < 0 {
                let io_err = io::Error::last_os_error();
                return Err(EventError::Enable(io_err));
            }
            Ok(())
        }
    }

    pub fn disable_ui_timer_event(&mut self) -> Result<(), EventError> {
        unsafe {
            let ui_timer_event = Kevent {
                ident: UI_TIMER_TICK,
                filter: kq::EVFILT_TIMER,
                flags: kq::EV_DELETE,
                fflags: 0,
                data: 0,
                udata: 0,
            };

            let changelist = [ui_timer_event];

            let result = kevent(
                self.queue,
                changelist.as_ptr(),
                changelist.len() as i32,
                ptr::null_mut(),
                0,
                ptr::null(),
            );

            if result < 0 {
                let io_err = io::Error::last_os_error();
                return Err(EventError::Enable(io_err));
            }
            Ok(())
        }
    }

    pub fn close(self) -> Result<(), EventError> {
        //TODO: Could this be drop instead?
        unsafe {
            let result = kq::close(self.queue);
            if result < 0 {
                let io_err = io::Error::last_os_error();
                Err(EventError::Close(io_err))
            } else {
                Ok(())
            }
        }
    }
}

//TODO: Refactor, think about abstractions that might make it a little easier
// to follow
pub fn build_event_queue() -> Result<(Sender, Receiver), EventError> {
    unsafe {
        // Create a new Kqueue
        let kqueue = kqueue();
        if kqueue < 0 {
            let io_err = io::Error::last_os_error();
            return Err(EventError::Create(io_err));
        }

        // Describe the events we are interested in...

        // New input available on stdin
        // TODO: See if EV_ENABLE is actually needed?
        let stdin_event = Kevent {
            ident: kq::STDIN_FILE_NUM,
            filter: kq::EVFILT_READ,
            flags: kq::EV_ADD | kq::EV_ENABLE,
            fflags: 0,
            data: 0,
            udata: 0,
        };

        // TODO: Maybe using a unique ident per file along with a EV_ONESHOT would be
        // easier? i.e using udata to signal the audio queue thats stopped

        // End of audio queue playback
        let playback_finished_event = Kevent {
            ident: AUDIO_QUEUE_PLAYBACK_FINISHED,
            filter: kq::EVFILT_USER,
            flags: kq::EV_ADD | kq::EV_CLEAR,
            fflags: 0,
            data: 0,
            udata: 0,
        };

        // Terminal resizing
        let terminal_resized_event = Kevent {
            ident: kq::SIGWINCH,
            filter: kq::EVFILT_SIGNAL,
            flags: kq::EV_ADD,
            fflags: 0,
            data: 0,
            udata: 0,
        };

        // Register interest in all events
        let changelist = [stdin_event, terminal_resized_event, playback_finished_event];

        let result = kevent(
            kqueue,
            changelist.as_ptr(),
            changelist.len() as i32,
            ptr::null_mut(),
            0,
            ptr::null(),
        );

        if result < 0 {
            let io_err = io::Error::last_os_error();
            return Err(EventError::Add(io_err));
        }

        let sender = Sender { queue: kqueue };

        let receiver = Receiver {
            queue: kqueue,
            queue_reader: KQueueReader::new(kqueue),
            input_reader: InputReader::new(kq::STDIN_FILE_NUM as i32),
        };

        Ok((sender, receiver))
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
                //TODO: Dont panic, expose error
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
                    //TODO: Dont panic, expose error
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
