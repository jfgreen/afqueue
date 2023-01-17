//! The afqueue module manages playback of a queue of audio files.
//!
//! Built on top of the macOS AudioToolbox framework.

// TODO: Diagram of how the different moving parts interact...

// TODO: How do we make sure this code isnt leaky over time?
// TODO: Use kAudioFilePropertyFormatList to deal with multi format files?
// TODO: Query the files channel layout to handle multi channel files?
// TODO: Check we dont orphan threads from one file to the next.
// TODO: Start consolidate things into abstractions

#![feature(extern_types)]

use std::cmp;
use std::ffi::{c_void, CStr, CString, NulError};
use std::fmt;
use std::io::{self, Write};
use std::mem::{self, MaybeUninit};
use std::ptr;

mod ffi;

use ffi::{
    AudioFileID, AudioQueueBufferRef, AudioQueueLevelMeterState, AudioQueuePropertyID,
    AudioQueueRef, Kqueue,
};

const LOWER_BUFFER_SIZE_HINT: u32 = 0x4000;
const UPPER_BUFFER_SIZE_HINT: u32 = 0x50000;
const BUFFER_SECONDS_HINT: f64 = 0.5;
const BUFFER_COUNT: usize = 3;

const AUDIO_QUEUE_PLAYBACK_FINISHED: u64 = 40;
const UI_TIMER_TICK: u64 = 41;

//TODO: Disable UI tick whilst paused
const UI_TICK_DURATION_MICROSECONDS: i64 = 33333;

type PacketPosition = i64;
type PacketCount = u32;

type PlaybackResult = Result<(), PlaybackError>;

#[derive(Debug)]
pub struct SystemErrorCode(i32);

impl fmt::Display for SystemErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let SystemErrorCode(code) = self;
        write!(f, "{code}")
    }
}

#[derive(Debug)]
pub enum PlaybackError {
    PathError(PathError),
    SystemError(SystemErrorCode),
    IOError(io::Error),
}

impl From<PathError> for PlaybackError {
    fn from(err: PathError) -> PlaybackError {
        PlaybackError::PathError(err)
    }
}

impl From<SystemErrorCode> for PlaybackError {
    fn from(err: SystemErrorCode) -> PlaybackError {
        // TODO: Map error codes to enum variants
        PlaybackError::SystemError(err)
    }
}

impl From<io::Error> for PlaybackError {
    fn from(err: io::Error) -> PlaybackError {
        PlaybackError::IOError(err)
    }
}

impl fmt::Display for PlaybackError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            PlaybackError::PathError(err) => {
                write!(f, "Supplied string is not a valid path: {}", err)
            }
            PlaybackError::SystemError(SystemErrorCode(code)) => {
                write!(f, "System error, code: '{}'", code)
            }
            PlaybackError::IOError(err) => {
                write!(f, "IO error: '{}'", err)
            }
        }
    }
}

#[derive(Debug)]
pub enum PathError {
    InteriorNull(NulError),
    PathIsEmpty,
}

impl From<NulError> for PathError {
    fn from(err: NulError) -> PathError {
        PathError::InteriorNull(err)
    }
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            PathError::InteriorNull(err) => {
                write!(f, "Path contained a null: {}", err)
            }
            PathError::PathIsEmpty => {
                write!(f, "Attempted to interpret an empty string as a path")
            }
        }
    }
}

type SystemResult<T> = Result<T, SystemErrorCode>;

enum Event {
    PauseKeyPressed,
    ExitKeyPressed,
    AudioQueueStopped,
    UITick,
}

pub fn start(paths: impl IntoIterator<Item = String>) -> PlaybackResult {
    let mut termios = read_current_termios()?;
    let original_termios = termios;
    enable_raw_mode(&mut termios);

    set_termios(&termios)?;
    print!("\x1b[?25l"); // Hide cursor

    let event_kqueue = build_event_kqueue()?;
    let mut event_reader = EventReader::new(event_kqueue);
    //TODO:: Nicer to have a "player" abstraction and to read off events here?
    for path in paths {
        print!("\x1b[2J"); // Clear screen
        print!("\x1b[1;1H"); // Position cursor at the top left
        play(&path, &mut event_reader, event_kqueue)?;
    }
    close_kqueue(event_kqueue)?;

    set_termios(&original_termios)?;

    print!("\x1b[2J"); // Clear screen
    print!("\x1b[1;1H"); // Position cursor at the top left
    print!("\x1b[?25h"); // Show cursor
    Ok(())
}

fn play(path: &str, event_reader: &mut EventReader, event_kqueue: Kqueue) -> PlaybackResult {
    //TODO: Double check the safety of sharing pointer to box to FFI thread
    // i.e ensure compiler wont helpfully free it too soon
    let playback_path = cstring_path(path)?;
    let playback_file = audio_file_open(&playback_path)?;
    let audio_metadata = audio_file_read_metadata(playback_file)?;
    let buffer_config = calculate_buffer_configuration(playback_file)?;
    let channel_count = buffer_config.format.channels_per_frame as usize;
    let playback_state = build_playback_state(playback_file, &buffer_config, event_kqueue);
    let playback_state = Box::new(playback_state);
    let state_ptr = Box::into_raw(playback_state) as *mut c_void;
    let output_queue = output_queue_create(&buffer_config.format, state_ptr)?;
    let buffers = create_buffers(output_queue, &buffer_config)?;

    audio_queue_listen_to_run_state(output_queue, event_kqueue)?;
    audio_queue_enable_metering(output_queue);

    if let Some(cookie) = audio_file_read_magic_cookie(playback_file)? {
        audio_queue_set_magic_cookie(output_queue, cookie)?
    }

    // While handle_buffer is usually invoked from the callback thread to refill a
    // buffer, we call it a few times before starting to pre load the buffers with
    // audio. This means that any error during pre-buffering is not directly
    // surfaced here, but reported back as if it was encountered on the callback
    // thread.
    for buffer_ref in buffers {
        // For small files, some buffers might remain unused.
        handle_buffer(state_ptr, output_queue, buffer_ref);
    }

    //TODO: Experiment with writing explocity to std out via std::io::stdout
    //TODO: Pull out UI stuff into a UI centric component
    print!("Properties:\r\n");
    for (k, v) in audio_metadata {
        print!("{k}: {v}\r\n");
    }

    //TODO: Test with channel count > 2 (figure out how we want to support this)
    // It looks like we might be able to read kAudioFilePropertyChannelLayout and
    // set kAudioQueueProperty_ChannelLayout

    // TODO: Might be interesting to see
    // what happens if we set the queue to mono but give it a stereo file
    print!("channel count: {}\r\n", channel_count);

    audio_queue_start(output_queue)?;

    //TODO: Debug mode for printing this stuff
    //println!("{buffer_config:?}");

    let mut paused = false;

    enable_ui_timer_event(event_kqueue, UI_TICK_DURATION_MICROSECONDS);

    //TODO: Make 'q' exit completely, and maybe 's' for skip

    //TODO: Would life just be easier if we returned an error
    //TODO: When we have finished playing... how to tell UIThread to stop?
    loop {
        let event = event_reader.next();

        match event {
            Event::PauseKeyPressed => {
                if paused {
                    audio_queue_start(output_queue)?;
                } else {
                    audio_queue_pause(output_queue)?;
                }
                paused = !paused;
            }
            Event::AudioQueueStopped => {
                audio_queue_dispose(output_queue, true)?;
                audio_file_close(playback_file)?;
                //TODO: Dont break - implement a tracklist
                break;
            }
            Event::ExitKeyPressed => {
                audio_queue_stop(output_queue, true)?;
            }
            Event::UITick => {
                //TODO: Explore different meters
                // amplitude over time, either wrap around or scroll
                // varios classic vu meters

                //TODO: Maintain a lock on stdout

                audio_queue_read_meter_level(output_queue, channel_count)?;

                //TODO: Dont ignore this error
                io::stdout().flush().unwrap();

                enable_ui_timer_event(event_kqueue, UI_TICK_DURATION_MICROSECONDS);
            }
        }
    }

    // Reset the playback finished event for re-use if there is another file to play
    enable_playback_finished_event(event_kqueue)?;

    // Rebox the state so it gets dropped
    // TODO: Test this works
    let _state = unsafe { Box::from_raw(state_ptr) };

    Ok(())
}

fn enable_playback_finished_event(kqueue: Kqueue) -> Result<(), io::Error> {
    unsafe {
        // Re enable the playback finished event
        let playback_finished_event = ffi::Kevent {
            ident: AUDIO_QUEUE_PLAYBACK_FINISHED,
            filter: ffi::EVFILT_USER,
            flags: ffi::EV_ENABLE,
            fflags: 0,
            data: 0,
            udata: 0,
        };

        let changelist = [playback_finished_event];

        // Register interest in both events
        let result = ffi::kevent(
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
fn enable_ui_timer_event(kqueue: Kqueue, usec: i64) {
    unsafe {
        let ui_timer_event = ffi::Kevent {
            ident: UI_TIMER_TICK,
            filter: ffi::EVFILT_TIMER,
            flags: ffi::EV_ADD | ffi::EV_ENABLE | ffi::EV_ONESHOT,
            fflags: ffi::NOTE_USECONDS,
            data: usec,
            udata: 0,
        };

        let changelist = [ui_timer_event];

        let result = ffi::kevent(
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
fn build_event_kqueue() -> Result<Kqueue, io::Error> {
    unsafe {
        let kqueue = ffi::kqueue();
        if kqueue < 0 {
            return Err(io::Error::last_os_error());
        }

        // TODO: See if EV_ENABLE is actually needed?

        // Describe the stdin events we are interested in
        let stdin_event = ffi::Kevent {
            ident: ffi::STDIN_FILE_NUM,
            filter: ffi::EVFILT_READ,
            flags: ffi::EV_ADD | ffi::EV_ENABLE,
            fflags: 0,
            data: 0,
            udata: 0,
        };

        // TODO: Maybe using a unique ident per file along with a EV_ONESHOT would be
        // easier?

        // Describe the playback finished events we are interested in
        // TODO: Increase confidence in using kqueue from one song to the next by using
        // udata to signal the audio queue thats stopped
        let playback_finished_event = ffi::Kevent {
            ident: AUDIO_QUEUE_PLAYBACK_FINISHED,
            filter: ffi::EVFILT_USER,
            flags: ffi::EV_ADD | ffi::EV_DISPATCH | ffi::EV_CLEAR,
            //flags: ffi::EV_ADD | ffi::EV_ONESHOT | ffi::EV_ENABLE,
            fflags: 0,
            data: 0,
            udata: 0,
        };

        let changelist = [stdin_event, playback_finished_event];

        // Register interest in both events
        let result = ffi::kevent(
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
        Ok(kqueue)
    }
}

fn close_kqueue(kqueue: Kqueue) -> Result<(), io::Error> {
    unsafe {
        let result = ffi::close(kqueue);
        if result < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

struct EventReader {
    queue: KQueueReader,
    input: InputReader,
}

impl EventReader {
    fn new(event_kqueue: Kqueue) -> Self {
        EventReader {
            queue: KQueueReader::new(event_kqueue),
            input: InputReader::new(ffi::STDIN_FILE_NUM as i32),
        }
    }

    fn next(&mut self) -> Event {
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
                (ffi::STDIN_FILE_NUM, ffi::EVFILT_READ) => {
                    self.input.fill_buffer();
                    continue;
                }
                (AUDIO_QUEUE_PLAYBACK_FINISHED, ffi::EVFILT_USER) => {
                    return Event::AudioQueueStopped
                }
                (UI_TIMER_TICK, ffi::EVFILT_TIMER) => return Event::UITick,
                _ => continue,
            }
        }
    }
}

const KEVENT_BUFFER_SIZE: usize = 10;
const INPUT_BUFFER_SIZE: usize = 10;

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

            let result = ffi::read(
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
    buffer: [ffi::Kevent; KEVENT_BUFFER_SIZE],
    kqueue: Kqueue,
    next: usize,
    filled: usize,
}

impl KQueueReader {
    fn new(kqueue: Kqueue) -> Self {
        KQueueReader {
            kqueue,
            buffer: [ffi::Kevent::default(); KEVENT_BUFFER_SIZE],
            next: 0,
            filled: 0,
        }
    }

    fn read(&mut self) -> ffi::Kevent {
        unsafe {
            if self.next == self.filled {
                let result = ffi::kevent(
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

#[derive(Debug)]
struct BufferConfiguration {
    format: ffi::AudioStreamBasicDescription,
    buffer_size: u32,
    packets_per_buffer: PacketCount,
    is_vbr: bool,
}

fn calculate_buffer_configuration(audio_file: AudioFileID) -> SystemResult<BufferConfiguration> {
    // Use
    //  - the theoretical max size of a packet of this format
    //  - some heuristics
    //  - a desired buffer duration (aproximate)
    // to determine
    // - how big each buffer needs to be
    // - how many packet to read each time we fill a buffer

    let format = audio_file_read_basic_description(audio_file)?;
    let max_packet_size = audio_file_read_packet_size_upper_bound(audio_file)?;
    let buffer_size = calculate_buffer_size(&format, max_packet_size);
    let is_vbr = format.bytes_per_packet == 0 || format.frames_per_packet == 0;
    let packets_per_buffer = buffer_size / max_packet_size;

    Ok(BufferConfiguration {
        format,
        buffer_size,
        packets_per_buffer,
        is_vbr,
    })
}

//TODO: Is this adding much value?
fn build_playback_state(
    audio_file: AudioFileID,
    buffer_config: &BufferConfiguration,
    event_queue: Kqueue,
) -> PlaybackState {
    let reader = if buffer_config.is_vbr {
        audio_file_read_vbr_packet_data
    } else {
        audio_file_read_cbr_packet_data
    };

    PlaybackState {
        event_queue,
        playback_file: audio_file,
        packets_per_buffer: buffer_config.packets_per_buffer,
        reader,
        current_packet: 0,
        finished: false,
    }
}

#[derive(Debug)]
struct PlaybackState {
    event_queue: Kqueue,
    playback_file: AudioFileID,
    reader: AudioFileReader,
    packets_per_buffer: PacketCount,
    current_packet: PacketPosition,
    finished: bool,
}

// TODO: Should always we ask for more packets than buffer can hold to ensure
// the buffer gets fully used?
//
// We could calculate the max packets per buffer instead of minimum? I.e
// optimistic instead of pessamistic.
//
// This would possibly take advantage of the properties AudioFileReadPacketData
// has over AudioFileReadPackets?
//
// We would still need an upper limit (and overallocate) the
// packet_descriptions.
//
// Is there somehow we could test this by detecting underutilized buffers?
// The queue could continue to callback with remaining buffers.
// Avoid unnecessary attempts to read the file again.

//TODO: Handle errors properly, send back to main thread somehow?

// This handler assumes `buffer` adheres to several invarients:
// - Is at least `packets_per_buffer` big
// - Were allocated with packet descriptions
// - Belong to `audio_queue`
extern "C" fn handle_buffer(
    user_data: *mut c_void,
    audio_queue: AudioQueueRef,
    buffer: AudioQueueBufferRef,
) {
    unsafe {
        let state = &mut *(user_data as *mut PlaybackState);

        if state.finished {
            return;
        }

        let read_result = (state.reader)(
            state.playback_file,
            state.current_packet,
            state.packets_per_buffer,
            buffer,
        );

        let packets_read = match read_result {
            //TODO: Report error properly
            Ok(packets_read) => packets_read,
            Err(error) => {
                print!("oh no: {error}\r\n");
                state.finished = true;
                return;
            }
        };

        if packets_read == 0 {
            state.finished = true;
            // Request an asynchronous stop so that buffered audio can finish playing.
            // Queue stopping is detected via seperate callback to property listener.
            //TODO: Handle Error
            audio_queue_stop(audio_queue, false);
            return;
        }

        match audio_queue_enqueue_buffer(audio_queue, buffer) {
            Ok(()) => {
                state.current_packet += packets_read as i64;
            }
            // Attempting to enqueue during reset can be expected when the user
            // has stopped the queue before playback has finished.
            Err(SystemErrorCode(ffi::AUDIO_QUEUE_ERROR_ENQUEUE_DURING_RESET)) => {
                state.finished = true;
            }
            // Anything else is probably a legitimate error condition
            Err(SystemErrorCode(code)) => {
                //TODO: Report error properly
                print!("oh no: {code}\r\n");
                state.finished = true;
            }
        }
    }
}

extern "C" fn handle_running_state_change(
    user_data: *mut c_void,
    audio_queue: AudioQueueRef,
    property: AudioQueuePropertyID,
) {
    // This handler should only react to changes to the "is running" property
    assert!(property == ffi::AUDIO_QUEUE_PROPERTY_IS_RUNNING);
    unsafe {
        let kqueue = user_data as Kqueue;

        match audio_queue_read_run_state(audio_queue) {
            Ok(0) => {
                //TODO: Extract up this and other deeply nested kqueue stuff into some helper
                // functions..
                let playback_finished_event = ffi::Kevent {
                    ident: AUDIO_QUEUE_PLAYBACK_FINISHED,
                    filter: ffi::EVFILT_USER,
                    flags: 0,
                    fflags: ffi::NOTE_TRIGGER,
                    data: 0,
                    udata: 0,
                };

                let changelist = [playback_finished_event];

                let result = ffi::kevent(
                    kqueue,
                    changelist.as_ptr(),
                    changelist.len() as i32,
                    ptr::null_mut(),
                    0,
                    ptr::null(),
                );

                if result < 0 {
                    //TODO: Better error messages!
                    panic!("oopsie: {}", io::Error::last_os_error());
                }
            }
            Ok(_) => {} // Ignore the queue starting
            //TODO: Feed back error to controller
            Err(error) => print!("booooo!: {error}\r\n"),
        }
    }
}

fn calculate_buffer_size(format: &ffi::AudioStreamBasicDescription, max_packet_size: u32) -> u32 {
    //TODO: Write some tests for this calculation (if we decide to keep it)
    if format.frames_per_packet != 0 {
        // If frames per packet are known, tailor the buffer size.
        let frames = format.sample_rate * BUFFER_SECONDS_HINT;
        let packets = (frames / (format.frames_per_packet as f64)).ceil() as u32;
        let size = packets * max_packet_size;
        let size = cmp::max(size, LOWER_BUFFER_SIZE_HINT);
        cmp::min(size, UPPER_BUFFER_SIZE_HINT)
    } else {
        // If frames per packet is not known, fallback to something large enough
        cmp::max(max_packet_size, UPPER_BUFFER_SIZE_HINT)
    }
}

fn create_buffers(
    output_queue: AudioQueueRef,
    buffer_config: &BufferConfiguration,
) -> SystemResult<Vec<AudioQueueBufferRef>> {
    let packet_descriptions = match buffer_config.is_vbr {
        true => buffer_config.packets_per_buffer,
        false => 0,
    };

    unsafe {
        vec![MaybeUninit::uninit(); BUFFER_COUNT]
            .into_iter()
            //TODO: Can we allocate buffers _without_ packet descriptions if we dont need them?
            .map(|mut buffer_ref| {
                let status = ffi::audio_queue_allocate_buffer_with_packet_descriptions(
                    output_queue,
                    buffer_config.buffer_size,
                    packet_descriptions,
                    buffer_ref.as_mut_ptr(),
                );

                if status == 0 {
                    Ok(buffer_ref.assume_init())
                } else {
                    Err(SystemErrorCode(status))
                }
            })
            .collect()
    }
}
type AudioFileReader = fn(
    file: AudioFileID,
    from_packet: PacketPosition,
    packets: PacketCount,
    buffer: AudioQueueBufferRef,
) -> SystemResult<PacketCount>;

fn audio_file_read_vbr_packet_data(
    file: AudioFileID,
    from_packet: PacketPosition,
    packets: PacketCount,
    buffer: AudioQueueBufferRef,
) -> SystemResult<PacketCount> {
    unsafe {
        let buffer = &mut *buffer;
        let mut num_bytes = buffer.audio_data_bytes_capacity;
        let mut num_packets = packets;

        let status = ffi::audio_file_read_packet_data(
            file,
            false, // dont use caching
            &mut num_bytes,
            buffer.packet_descriptions,
            from_packet,
            &mut num_packets,
            buffer.audio_data,
        );

        if status != 0 {
            return Err(SystemErrorCode(status));
        }

        buffer.audio_data_byte_size = num_bytes;
        buffer.packet_description_count = num_packets;

        Ok(num_packets)
    }
}

fn audio_file_read_cbr_packet_data(
    file: AudioFileID,
    from_packet: PacketPosition,
    packets: PacketCount,
    buffer: AudioQueueBufferRef,
) -> SystemResult<PacketCount> {
    unsafe {
        let buffer = &mut *buffer;
        let mut num_bytes = buffer.audio_data_bytes_capacity;
        let mut num_packets = packets;

        let status = ffi::audio_file_read_packet_data(
            file,
            false, // dont use caching
            &mut num_bytes,
            ptr::null_mut(),
            from_packet,
            &mut num_packets,
            buffer.audio_data,
        );

        if status != 0 {
            return Err(SystemErrorCode(status));
        }

        buffer.audio_data_byte_size = num_bytes;
        buffer.packet_description_count = 0;

        Ok(num_packets)
    }
}

fn cstring_path(path: &str) -> Result<CString, PathError> {
    if path.is_empty() {
        return Err(PathError::PathIsEmpty);
    }

    Ok(CString::new(path)?)
}

fn audio_file_open(path: &CStr) -> SystemResult<AudioFileID> {
    let path = path.to_bytes();

    unsafe {
        // Create URL
        let url_ref = ffi::cfurl_create_from_filesystem_representation(
            ptr::null(), // Use default allocator
            path.as_ptr(),
            path.len() as isize,
            false, // Not a directory
        );

        // Open file
        let mut file_id = MaybeUninit::uninit();
        let status = ffi::audio_file_open_url(
            url_ref,
            ffi::AUDIO_FILE_READ_PERMISSION,
            0, // No file hints
            file_id.as_mut_ptr(),
        );

        // Dont need the CFURL anymore
        ffi::cf_release(url_ref as *const c_void);

        if status != 0 {
            return Err(SystemErrorCode(status));
        }

        let file_id = file_id.assume_init();
        Ok(file_id)
    }
}

fn audio_file_close(file: AudioFileID) -> SystemResult<()> {
    unsafe {
        let status = ffi::audio_file_close(file);
        if status != 0 {
            return Err(SystemErrorCode(status));
        }
    }
    Ok(())
}

fn audio_file_read_metadata(file: AudioFileID) -> SystemResult<Vec<(String, String)>> {
    unsafe {
        let info_dict = audio_file_get_property(file, ffi::AUDIO_FILE_PROPERTY_INFO_DICTIONARY)?;

        // Extract keys and values
        let count = ffi::cfdictionary_get_count(info_dict);
        let mut keys = vec![0 as ffi::CFStringRef; count as usize];
        let mut values = vec![0 as ffi::CFStringRef; count as usize];

        ffi::cfdictionary_get_keys_and_values(
            info_dict,
            keys.as_mut_ptr() as *mut *const c_void,
            values.as_mut_ptr() as *mut *const c_void,
        );

        // Filter out non CFString values and convert to Rust strings
        // Note: We eagerly collect to force conversation before the dictionary is
        // released

        let cfstring_type_id = ffi::cfstring_get_type_id();

        let properties = keys
            .into_iter()
            .zip(values.into_iter())
            .filter(|(_, v)| ffi::cf_get_type_id(*v as *const c_void) == cfstring_type_id)
            .map(|(k, v)| (cfstring_to_string(k), cfstring_to_string(v)))
            .collect();

        ffi::cf_release(info_dict as *const c_void);

        Ok(properties)
    }
}

fn audio_file_read_basic_description(
    file: AudioFileID,
) -> SystemResult<ffi::AudioStreamBasicDescription> {
    audio_file_get_property(file, ffi::AUDIO_FILE_PROPERTY_DATA_FORMAT)
}

fn audio_file_read_packet_size_upper_bound(file: AudioFileID) -> SystemResult<u32> {
    audio_file_get_property(file, ffi::AUDIO_FILE_PROPERTY_PACKET_SIZE_UPPER_BOUND)
}

// This only works with sized types
fn audio_file_get_property<T>(
    file_id: ffi::AudioFileID,
    property: ffi::AudioFilePropertyID,
) -> SystemResult<T> {
    unsafe {
        let mut data = MaybeUninit::<T>::uninit();
        let mut data_size = mem::size_of::<T>() as u32;

        let status = ffi::audio_file_get_property(
            file_id,
            property,
            &mut data_size as *mut _,
            data.as_mut_ptr() as *mut c_void,
        );
        let data = data.assume_init();

        if status != 0 {
            return Err(SystemErrorCode(status));
        }

        // audio_file_get_property outputs the number of bytes written to data_size
        // Check to see if this is correct for the given type
        assert!(data_size == mem::size_of::<T>() as u32);

        Ok(data)
    }
}

fn audio_file_read_magic_cookie(file: AudioFileID) -> SystemResult<Option<Vec<u8>>> {
    unsafe {
        // Check to see if there is a cookie, and if so how large it is.
        let mut cookie_size: u32 = 0;
        let mut is_writable: u32 = 0;
        let status = ffi::audio_file_get_property_info(
            file,
            ffi::AUDIO_FILE_PROPERTY_MAGIC_COOKIE_DATA,
            &mut cookie_size as *mut _,
            &mut is_writable as *mut _,
        );

        // No magic cookie data
        if status == ffi::AUDIO_FILE_ERROR_UNSUPPORTED_PROPERTY {
            return Ok(None);
        }

        // Some other status is probably an error
        if status != 0 {
            return Err(SystemErrorCode(status));
        }

        // Read the cookie
        let mut cookie_data: Vec<u8> = vec![0; cookie_size as usize];
        let mut data_size = cookie_size;

        let status = ffi::audio_file_get_property(
            file,
            ffi::AUDIO_FILE_PROPERTY_MAGIC_COOKIE_DATA,
            &mut data_size as *mut _,
            cookie_data.as_mut_ptr() as *mut c_void,
        );

        if status != 0 {
            return Err(SystemErrorCode(status));
        }

        assert!(data_size == cookie_size);
        Ok(Some(cookie_data))
    }
}

fn output_queue_create(
    format: *const ffi::AudioStreamBasicDescription,
    user_data: *mut c_void,
) -> SystemResult<AudioQueueRef> {
    unsafe {
        let mut output_queue = MaybeUninit::uninit();
        let status = ffi::audio_queue_new_output(
            format,
            handle_buffer,
            user_data,
            0 as *const _, // Run loop
            0 as *const _, // Run loop mode
            0,             // flags
            output_queue.as_mut_ptr(),
        );

        if status != 0 {
            return Err(SystemErrorCode(status));
        }
        let output_queue = output_queue.assume_init();
        Ok(output_queue)
    }
}

fn audio_queue_set_magic_cookie(queue: AudioQueueRef, cookie: Vec<u8>) -> SystemResult<()> {
    unsafe {
        let status = ffi::audio_queue_set_property(
            queue,
            ffi::AUDIO_QUEUE_PROPERTY_MAGIC_COOKIE_DATA,
            cookie.as_ptr() as *const c_void,
            cookie.len() as u32,
        );

        if status == 0 {
            Ok(())
        } else {
            Err(SystemErrorCode(status))
        }
    }
}

fn audio_queue_start(queue: AudioQueueRef) -> SystemResult<()> {
    unsafe {
        let status = ffi::audio_queue_start(queue, ptr::null());

        if status == 0 {
            Ok(())
        } else {
            Err(SystemErrorCode(status))
        }
    }
}

fn audio_queue_stop(queue: AudioQueueRef, immediate: bool) -> SystemResult<()> {
    unsafe {
        let status = ffi::audio_queue_stop(queue, immediate);

        if status == 0 {
            Ok(())
        } else {
            Err(SystemErrorCode(status))
        }
    }
}

fn audio_queue_dispose(queue: AudioQueueRef, immediate: bool) -> SystemResult<()> {
    unsafe {
        let status = ffi::audio_queue_dispose(queue, immediate);

        if status == 0 {
            Ok(())
        } else {
            Err(SystemErrorCode(status))
        }
    }
}

fn audio_queue_pause(queue: AudioQueueRef) -> SystemResult<()> {
    unsafe {
        let status = ffi::audio_queue_pause(queue);

        if status == 0 {
            Ok(())
        } else {
            Err(SystemErrorCode(status))
        }
    }
}

fn audio_queue_enqueue_buffer(
    queue: AudioQueueRef,
    buffer: AudioQueueBufferRef,
) -> SystemResult<()> {
    unsafe {
        let status = ffi::audio_queue_enqueue_buffer(
            queue,
            buffer,
            // Packet descriptions are supplied via buffer itself
            0,
            ptr::null(),
        );

        if status == 0 {
            Ok(())
        } else {
            Err(SystemErrorCode(status))
        }
    }
}

fn audio_queue_listen_to_run_state(queue: AudioQueueRef, kqueue: Kqueue) -> SystemResult<()> {
    unsafe {
        let status = ffi::audio_queue_add_property_listener(
            queue,
            ffi::AUDIO_QUEUE_PROPERTY_IS_RUNNING,
            handle_running_state_change,
            kqueue as *mut c_void,
        );
        if status == 0 {
            Ok(())
        } else {
            Err(SystemErrorCode(status))
        }
    }
}

fn audio_queue_read_run_state(queue: AudioQueueRef) -> SystemResult<u32> {
    unsafe {
        let mut data = MaybeUninit::<u32>::uninit();
        let mut data_size = mem::size_of::<u32>() as u32;

        let status = ffi::audio_queue_get_property(
            queue,
            ffi::AUDIO_QUEUE_PROPERTY_IS_RUNNING,
            data.as_mut_ptr() as *mut c_void,
            &mut data_size as *mut _,
        );

        let data = data.assume_init();

        if status != 0 {
            return Err(SystemErrorCode(status));
        }

        // audio_queue_get_property outputs the number of bytes written to data_size
        // Check to see if this is correct
        assert!(data_size == mem::size_of::<u32>() as u32);

        Ok(data)
    }
}

fn audio_queue_enable_metering(queue: AudioQueueRef) -> SystemResult<()> {
    unsafe {
        let enabled: u32 = 1;

        let status = ffi::audio_queue_set_property(
            queue,
            ffi::AUDIO_QUEUE_PROPERTY_ENABLE_LEVEL_METERING,
            &enabled as *const _ as *const c_void,
            mem::size_of::<u32>() as u32,
        );

        if status == 0 {
            Ok(())
        } else {
            Err(SystemErrorCode(status))
        }
    }
}

fn audio_queue_read_meter_level(
    queue: AudioQueueRef,
    channel_count: usize,
) -> SystemResult<Vec<AudioQueueLevelMeterState>> {
    //TODO: Figure out how to return slice
    //TODO: Extract audio_queue_get_property function for this and
    // audio_queue_read_run_state. TODO: Support stereo
    unsafe {
        let mut meter_state: Vec<AudioQueueLevelMeterState> = Vec::with_capacity(channel_count);
        let expected_size = (mem::size_of::<AudioQueueLevelMeterState>() * channel_count) as u32;
        let mut data_size = expected_size;

        let status = ffi::audio_queue_get_property(
            queue,
            ffi::AUDIO_QUEUE_PROPERTY_LEVEL_METER_STATE,
            meter_state.as_mut_ptr() as *mut c_void,
            &mut data_size as *mut _,
        );

        if status != 0 {
            return Err(SystemErrorCode(status));
        }

        assert!(data_size == expected_size);
        meter_state.set_len(channel_count);

        //TODO: Test UI works when going from mono file to stereo file

        //FIXME: The following UI code should not live here

        //TODO: Get max bar length from term
        let max_bar_length: f32 = 100.0;

        for channel in meter_state.iter() {
            print!("\r\n");
            let bar_length = (max_bar_length * channel.average_power) as usize;
            for _ in 0..bar_length {
                print!("█")
            }
            print!("\x1b[K"); // Clear remainder of line
        }
        print!("\x1b[{}A", channel_count); // Hop back up to channel

        Ok(meter_state)
    }
}

unsafe fn cfstring_to_string(cfstring: ffi::CFStringRef) -> String {
    assert!(!cfstring.is_null());

    let string_len = ffi::cfstring_get_length(cfstring);

    // This is effectively asking how big a buffer we are going to need
    let mut bytes_required = 0;
    ffi::cfstring_get_bytes(
        cfstring,
        ffi::CFRange {
            location: 0,
            length: string_len,
        },
        ffi::CFSTRING_ENCODING_UTF8,
        0,               // no loss byte
        false,           // no byte order marker
        ptr::null_mut(), // dont actually capture any bytes
        0,               // buffer size of 0 as no buffer supplied
        &mut bytes_required,
    );

    // Now actually copy out the bytes
    let mut buffer = vec![b'\x00'; bytes_required as usize];
    let mut bytes_written = 0;

    let chars_converted = ffi::cfstring_get_bytes(
        cfstring,
        ffi::CFRange {
            location: 0,
            length: string_len,
        },
        ffi::CFSTRING_ENCODING_UTF8,
        0,     // no loss byte
        false, // no byte order marker
        buffer.as_mut_ptr(),
        buffer.len() as ffi::CFIndex,
        &mut bytes_written,
    );

    assert!(chars_converted == string_len);
    assert!(bytes_written as usize == buffer.len());

    String::from_utf8_unchecked(buffer)
}

fn enable_raw_mode(termios: &mut ffi::Termios) {
    // Disable echoing
    termios.c_lflag &= !ffi::ECHO;

    // Read input byte by byte instead of line by line
    termios.c_lflag &= !ffi::ICANON;

    // Disable Ctrl-C and Ctrl-Z signals
    termios.c_lflag &= !ffi::ISIG;

    // Disable Ctrl-S and Ctrl-Q flow control
    termios.c_iflag &= !ffi::IXON;

    // Disable Ctrl-V (literal quoting) and Ctrl-O (discard pending)
    termios.c_lflag &= !ffi::IEXTEN;

    // Fix Ctrl-M by disabling translation of carriage return to newlines
    termios.c_iflag &= !ffi::ICRNL;

    // Disable adding a carriage raturn to each outputed newline
    termios.c_oflag &= !ffi::OPOST;

    // Disable break condition causing sigint
    termios.c_iflag &= !ffi::BRKINT;

    // NOTE: disabling INPCK, ISTRIP, and enabling CS8
    // are traditionally part of setting up "raw terminal output".
    // However, this aleady seems to be the case for Terminal.app
}

fn read_current_termios() -> Result<ffi::Termios, io::Error> {
    unsafe {
        let mut termios = MaybeUninit::uninit();
        let result = ffi::tcgetattr(ffi::STDIN_FILE_NUM as i32, termios.as_mut_ptr());
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(termios.assume_init())
    }
}

fn set_termios(termios: &ffi::Termios) -> Result<(), io::Error> {
    unsafe {
        let result = ffi::tcsetattr(ffi::STDIN_FILE_NUM as i32, ffi::TCSAFLUSH, termios);
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}
