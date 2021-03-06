//! The afqueue module manages playback of a queue of audio files.
//!
//! Built on top of the macOS AudioToolbox framework.

// TODO: Diagram of how the different threads interact...

// TODO: How do we make sure this code isnt leaky over time?
// TODO: Use kAudioFilePropertyFormatList to deal with multi format files?
// TODO: Query the files channel layout to handle multi channel files?
// TODO: Check we dont orphan threads from one file to the next.

#![feature(extern_types)]

use std::cmp;
use std::ffi::{c_void, CStr, CString, NulError};
use std::fmt;
use std::mem::{self, MaybeUninit};
use std::ptr;
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};

mod system;

use system::{self as sys, AudioFileID, AudioQueueBufferRef, AudioQueuePropertyID, AudioQueueRef};

const LOWER_BUFFER_SIZE_HINT: u32 = 0x4000;
const UPPER_BUFFER_SIZE_HINT: u32 = 0x50000;
const BUFFER_SECONDS_HINT: f64 = 0.5;
const BUFFER_COUNT: usize = 3;

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

impl fmt::Display for PlaybackError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            PlaybackError::PathError(err) => {
                write!(f, "Supplied string is not a valid path: {}", err)
            }
            PlaybackError::SystemError(SystemErrorCode(code)) => {
                write!(f, "System error, code: '{}'", code)
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
}

pub fn start(paths: impl IntoIterator<Item = String>) -> PlaybackResult {
    let path = paths.into_iter().next().unwrap();
    play(&path)
}

fn play(path: &str) -> PlaybackResult {
    let (events_tx, events_rx) = mpsc::channel();
    let user_input_handler = launch_user_input_handler(events_tx.clone());

    //TODO: Double check the safety of sharing pointer to box to FFI thread
    // i.e ensure compiler wont helpfully free it too soon
    let playback_path = cstring_path(path)?;
    let playback_file = audio_file_open(&playback_path)?;
    let audio_metadata = audio_file_read_metadata(playback_file)?;
    let buffer_config = calculate_buffer_configuration(playback_file)?;
    let playback_state = build_playback_state(playback_file, &buffer_config, events_tx.clone());
    let playback_state = Box::new(playback_state);
    let state_ptr = Box::into_raw(playback_state) as *mut c_void;
    let output_queue = output_queue_create(&buffer_config.format, state_ptr)?;
    let buffers = create_buffers(output_queue, &buffer_config)?;

    let state_listener_events_tx = Box::new(events_tx.clone());
    let state_listener_events_tx = Box::into_raw(state_listener_events_tx);
    audio_queue_listen_to_run_state(output_queue, state_listener_events_tx)?;

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

    println!("Properties:");
    for (k, v) in audio_metadata {
        println!("{k}: {v}");
    }

    audio_queue_start(output_queue)?;

    println!("{buffer_config:?}");

    let mut paused = false;

    //TODO: Would life just be easier if we returned an error
    //TODO: When we have finished playing... how to tell UIThread to stop?
    loop {
        let event = events_rx
            .recv()
            .expect("Failed to read event: channel unexpectedly closed");

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
                println!("disposing");
                audio_queue_dispose(output_queue, true)?;
                println!("closing");
                audio_file_close(playback_file)?;
                //TODO: Dont break - implement a tracklist
                break;
            }
            Event::ExitKeyPressed => {
                audio_queue_stop(output_queue, true)?;
            }
        }
    }

    println!("joining");
    user_input_handler
        .join()
        .expect("Panic in input handling thread");

    // Rebox the state so it gets dropped
    // TODO: Test this works
    let _state = unsafe { Box::from_raw(state_ptr) };
    let _listener_tx = unsafe { Box::from_raw(state_listener_events_tx) };

    Ok(())
}

fn launch_user_input_handler(events_tx: Sender<Event>) -> JoinHandle<()> {
    thread::spawn(move || {
        //TODO: Implement raw terminal IO
        let send = |e| {
            events_tx
                .send(e)
                .expect("Failed to send event: channel unexpectedly closed")
        };

        let mut input = String::new();

        loop {
            input.clear();
            std::io::stdin().read_line(&mut input).unwrap();
            match input.trim().to_lowercase().as_str() {
                "q" => {
                    send(Event::ExitKeyPressed);
                    break;
                }
                "p" => {
                    send(Event::PauseKeyPressed);
                }
                _ => {}
            }
        }
    })
}

#[derive(Debug)]
struct BufferConfiguration {
    format: sys::AudioStreamBasicDescription,
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
    events: Sender<Event>,
) -> PlaybackState {
    let reader = if buffer_config.is_vbr {
        audio_file_read_vbr_packet_data
    } else {
        audio_file_read_cbr_packet_data
    };

    PlaybackState {
        events,
        playback_file: audio_file,
        packets_per_buffer: buffer_config.packets_per_buffer,
        reader,
        current_packet: 0,
        finished: false,
    }
}

#[derive(Debug)]
struct PlaybackState {
    events: Sender<Event>,
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
            println!("ignoring request to fill buffer");
            return;
        }

        let read_result = (state.reader)(
            state.playback_file,
            state.current_packet,
            state.packets_per_buffer,
            buffer,
        );

        println!("{read_result:?}");
        let packets_read = match read_result {
            //TODO: Report error properly
            Ok(packets_read) => packets_read,
            Err(error) => {
                println!("oh no: {error}");
                state.finished = true;
                return;
            }
        };

        if packets_read == 0 {
            state.finished = true;
            println!("end of file");
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
            Err(SystemErrorCode(sys::AUDIO_QUEUE_ERROR_ENQUEUE_DURING_RESET)) => {
                state.finished = true;
            }
            // Anything else is probably a legitimate error condition
            Err(SystemErrorCode(code)) => {
                //TODO: Report error properly
                println!("oh no: {code}");
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
    assert!(property == sys::AUDIO_QUEUE_PROPERTY_IS_RUNNING);
    unsafe {
        let events_tx = &mut *(user_data as *mut Sender<Event>);

        match audio_queue_read_run_state(audio_queue) {
            Ok(0) => {
                (events_tx)
                    .send(Event::AudioQueueStopped)
                    .expect("Failed to send event: channel unexpectedly closed");
            }
            Ok(_) => {} // Ignore the queue starting
            //TODO: Feed back error to controller
            Err(error) => println!("booooo!: {error}"),
        }
    }
}

fn calculate_buffer_size(format: &sys::AudioStreamBasicDescription, max_packet_size: u32) -> u32 {
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
                let status = sys::audio_queue_allocate_buffer_with_packet_descriptions(
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

        let status = sys::audio_file_read_packet_data(
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

        let status = sys::audio_file_read_packet_data(
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
        let url_ref = sys::cfurl_create_from_filesystem_representation(
            ptr::null(), // Use default allocator
            path.as_ptr(),
            path.len() as isize,
            false, // Not a directory
        );

        // Open file
        let mut file_id = MaybeUninit::uninit();
        let status = sys::audio_file_open_url(
            url_ref,
            sys::AUDIO_FILE_READ_PERMISSION,
            0, // No file hints
            file_id.as_mut_ptr(),
        );

        // Dont need the CFURL anymore
        sys::cf_release(url_ref as *const c_void);

        if status != 0 {
            return Err(SystemErrorCode(status));
        }

        let file_id = file_id.assume_init();
        Ok(file_id)
    }
}

fn audio_file_close(file: AudioFileID) -> SystemResult<()> {
    unsafe {
        let status = sys::audio_file_close(file);
        if status != 0 {
            return Err(SystemErrorCode(status));
        }
    }
    Ok(())
}

fn audio_file_read_metadata(file: AudioFileID) -> SystemResult<Vec<(String, String)>> {
    unsafe {
        let info_dict = audio_file_get_property(file, sys::AUDIO_FILE_PROPERTY_INFO_DICTIONARY)?;

        // Extract keys and values
        let count = sys::cfdictionary_get_count(info_dict);
        let mut keys = vec![0 as sys::CFStringRef; count as usize];
        let mut values = vec![0 as sys::CFStringRef; count as usize];

        sys::cfdictionary_get_keys_and_values(
            info_dict,
            keys.as_mut_ptr() as *mut *const c_void,
            values.as_mut_ptr() as *mut *const c_void,
        );

        // Filter out non CFString values and convert to Rust strings
        // Note: We eagerly collect to force conversation before the dictionary is
        // released

        let cfstring_type_id = sys::cfstring_get_type_id();

        let properties = keys
            .into_iter()
            .zip(values.into_iter())
            .filter(|(_, v)| sys::cf_get_type_id(*v as *const c_void) == cfstring_type_id)
            .map(|(k, v)| (cfstring_to_string(k), cfstring_to_string(v)))
            .collect();

        sys::cf_release(info_dict as *const c_void);

        Ok(properties)
    }
}

fn audio_file_read_basic_description(
    file: AudioFileID,
) -> SystemResult<sys::AudioStreamBasicDescription> {
    audio_file_get_property(file, sys::AUDIO_FILE_PROPERTY_DATA_FORMAT)
}

fn audio_file_read_packet_size_upper_bound(file: AudioFileID) -> SystemResult<u32> {
    audio_file_get_property(file, sys::AUDIO_FILE_PROPERTY_PACKET_SIZE_UPPER_BOUND)
}

// This only works with sized types
fn audio_file_get_property<T>(
    file_id: sys::AudioFileID,
    property: sys::AudioFilePropertyID,
) -> SystemResult<T> {
    unsafe {
        let mut data = MaybeUninit::<T>::uninit();
        let mut data_size = mem::size_of::<T>() as u32;

        let status = sys::audio_file_get_property(
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
        let status = sys::audio_file_get_property_info(
            file,
            sys::AUDIO_FILE_PROPERTY_MAGIC_COOKIE_DATA,
            &mut cookie_size as *mut _,
            &mut is_writable as *mut _,
        );

        // No magic cookie data
        if status == sys::AUDIO_FILE_ERROR_UNSUPPORTED_PROPERTY {
            return Ok(None);
        }

        // Some other status is probably an error
        if status != 0 {
            return Err(SystemErrorCode(status));
        }

        // Read the cookie
        let mut cookie_data: Vec<u8> = vec![0; cookie_size as usize];
        let mut data_size = cookie_size;

        let status = sys::audio_file_get_property(
            file,
            sys::AUDIO_FILE_PROPERTY_MAGIC_COOKIE_DATA,
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
    format: *const sys::AudioStreamBasicDescription,
    user_data: *mut c_void,
) -> SystemResult<AudioQueueRef> {
    unsafe {
        let mut output_queue = MaybeUninit::uninit();
        let status = sys::audio_queue_new_output(
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
        let status = sys::audio_queue_set_property(
            queue,
            system::AUDIO_QUEUE_PROPERTY_MAGIC_COOKIE_DATA,
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
        let status = sys::audio_queue_start(queue, ptr::null());

        if status == 0 {
            Ok(())
        } else {
            Err(SystemErrorCode(status))
        }
    }
}

fn audio_queue_stop(queue: AudioQueueRef, immediate: bool) -> SystemResult<()> {
    unsafe {
        let status = sys::audio_queue_stop(queue, immediate);

        if status == 0 {
            Ok(())
        } else {
            Err(SystemErrorCode(status))
        }
    }
}

fn audio_queue_dispose(queue: AudioQueueRef, immediate: bool) -> SystemResult<()> {
    unsafe {
        let status = sys::audio_queue_dispose(queue, immediate);

        if status == 0 {
            Ok(())
        } else {
            Err(SystemErrorCode(status))
        }
    }
}

fn audio_queue_pause(queue: AudioQueueRef) -> SystemResult<()> {
    unsafe {
        let status = sys::audio_queue_pause(queue);

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
        let status = sys::audio_queue_enqueue_buffer(
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

fn audio_queue_listen_to_run_state(
    queue: AudioQueueRef,
    events_tx: *const Sender<Event>,
) -> SystemResult<()> {
    unsafe {
        let status = sys::audio_queue_add_property_listener(
            queue,
            sys::AUDIO_QUEUE_PROPERTY_IS_RUNNING,
            handle_running_state_change,
            events_tx as *mut c_void,
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

        let status = sys::audio_queue_get_property(
            queue,
            sys::AUDIO_QUEUE_PROPERTY_IS_RUNNING,
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

unsafe fn cfstring_to_string(cfstring: sys::CFStringRef) -> String {
    assert!(!cfstring.is_null());

    let string_len = sys::cfstring_get_length(cfstring);

    // This is effectively asking how big a buffer we are going to need
    let mut bytes_required = 0;
    sys::cfstring_get_bytes(
        cfstring,
        sys::CFRange {
            location: 0,
            length: string_len,
        },
        sys::CFSTRING_ENCODING_UTF8,
        0,               // no loss byte
        false,           // no byte order marker
        ptr::null_mut(), // dont actually capture any bytes
        0,               // buffer size of 0 as no buffer supplied
        &mut bytes_required,
    );

    // Now actually copy out the bytes
    let mut buffer = vec![b'\x00'; bytes_required as usize];
    let mut bytes_written = 0;

    let chars_converted = sys::cfstring_get_bytes(
        cfstring,
        sys::CFRange {
            location: 0,
            length: string_len,
        },
        sys::CFSTRING_ENCODING_UTF8,
        0,     // no loss byte
        false, // no byte order marker
        buffer.as_mut_ptr(),
        buffer.len() as sys::CFIndex,
        &mut bytes_written,
    );

    assert!(chars_converted == string_len);
    assert!(bytes_written as usize == buffer.len());

    String::from_utf8_unchecked(buffer)
}
