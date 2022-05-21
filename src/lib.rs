//! The afqueue module manages playback of a queue of audio files.
//!
//! Built on top of the macOS AudioToolbox framework.

#![feature(extern_types)]

use std::cmp;
use std::ffi::{c_void, CString, NulError};
use std::fmt;
use std::mem::{self, MaybeUninit};
use std::ptr;

mod system;

use system::{
    self as sys, AudioFileID, AudioQueueBufferRef as BufferRef, AudioQueueRef as QueueRef,
};

const LOWER_BUFFER_SIZE_HINT: u32 = 0x4000;
const UPPER_BUFFER_SIZE_HINT: u32 = 0x50000;
const BUFFER_SECONDS_HINT: f64 = 0.5;
const BUFFER_COUNT: usize = 3;

pub type SystemErrorCode = i32;

type PlaybackResult<T> = Result<T, PlaybackError>;
type PacketPosition = i64;
type PacketCount = u32;

pub enum PlaybackError {
    PathContainsInteriorNull(NulError),
    PathIsEmpty,
    FailedToOpenAudioFile(SystemErrorCode),
    FailedToCloseAudioFile(SystemErrorCode),
    FailedToReadFileProperty(SystemErrorCode),
    FailedToReadFilePropertyInfo(SystemErrorCode),
    FailedToCreateAudioQueue(SystemErrorCode),
    FailedToSetAudioQueueProperty(SystemErrorCode),
    FailedToAllocateBuffer(SystemErrorCode),
    FailedToStartAudioQueue(SystemErrorCode),
    FailedToStopAudioQueue(SystemErrorCode),
    FailedToDisposeAudioQueue(SystemErrorCode),
    FailedToReadFromAudioFile(SystemErrorCode),
    FailedToEnqueueBuffer(SystemErrorCode),
}

impl From<NulError> for PlaybackError {
    fn from(err: NulError) -> PlaybackError {
        PlaybackError::PathContainsInteriorNull(err)
    }
}

impl fmt::Display for PlaybackError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            PlaybackError::PathContainsInteriorNull(err) => {
                write!(f, "Path contained a null: {}", err)
            }
            PlaybackError::PathIsEmpty => {
                write!(f, "Attempted to interpret an empty string as a path")
            }
            PlaybackError::FailedToOpenAudioFile(code) => {
                write!(f, "Failed to open audio file, error: {}", code)
            }
            PlaybackError::FailedToCloseAudioFile(code) => {
                write!(f, "Failed to close audio file, error: {}", code)
            }
            PlaybackError::FailedToReadFileProperty(code) => {
                write!(f, "Failed to read file property, error: {}", code)
            }
            PlaybackError::FailedToReadFilePropertyInfo(code) => {
                write!(f, "Failed to read file property, error: {}", code)
            }
            PlaybackError::FailedToCreateAudioQueue(code) => {
                write!(f, "Failed to create audio queue, error: {}", code)
            }
            PlaybackError::FailedToSetAudioQueueProperty(code) => {
                write!(f, "Failed to set audio queue property, error: {}", code)
            }
            PlaybackError::FailedToAllocateBuffer(code) => {
                write!(f, "Failed to allocate buffer, error: {}", code)
            }
            PlaybackError::FailedToStartAudioQueue(code) => {
                write!(f, "Failed to start audio queue, error: {}", code)
            }
            PlaybackError::FailedToStopAudioQueue(code) => {
                write!(f, "Failed to stop audio queue, error: {}", code)
            }
            PlaybackError::FailedToDisposeAudioQueue(code) => {
                write!(f, "Failed to dispose audio queue, error: {}", code)
            }
            PlaybackError::FailedToReadFromAudioFile(code) => {
                write!(f, "Failed to read from audio file, error: {}", code)
            }
            PlaybackError::FailedToEnqueueBuffer(code) => {
                write!(f, "Failed to enqueue buffer, error: {}", code)
            }
        }
    }
}

// TODO: How do we make sure this code isnt leaky over time?
// TODO: Use kAudioFilePropertyFormatList to deal with multi format files?
// TODO: Query the files channel layout to handle multi channel files?

pub fn play(path: String) -> PlaybackResult<()> {
    let playback_file = audio_file_open(path)?;

    println!("Properties:");
    for (k, v) in audio_file_read_properties(playback_file)? {
        println!("{k}: {v}");
    }

    //TODO: Maybe buffer config can be a builder afterall?...
    let buffer_config = BufferConfig::from_audio_file(playback_file)?;
    let reader = AudioFileReader::new(playback_file, &buffer_config);
    let mut handler = PlaybackHandler::new(reader);
    let handler_ptr = &mut handler as *mut _ as *mut c_void;
    let format_ptr = &buffer_config.format as *const _;
    let output_queue = output_queue_create(format_ptr, handler_ptr)?;
    let buffers = create_buffers(output_queue, &buffer_config)?;

    if let Some(cookie) = audio_file_read_magic_cookie(playback_file)? {
        println!("Magic cookie is {} bytes", cookie.len());
        output_queue_set_magic_cookie(output_queue, cookie)?;
    }

    // Pre load buffers with audio
    // For small files, this might result only some of the buffers being enqueued.
    for buffer in buffers {
        handler.enqueue_next_buffer(output_queue, buffer)?;
    }

    println!("Main thread id: {:?}", std::thread::current().id());

    output_queue_start(output_queue)?;

    //FIXME: Create real controls
    let mut input = String::new();
    std::io::stdin().read_line(&mut input).unwrap();

    output_queue_stop(output_queue, true)?;
    output_queue_dispose(output_queue, true)?;

    audio_file_close(playback_file)?;
    Ok(())
}

struct PlaybackHandler {
    reader: AudioFileReader,
    current_packet: PacketPosition,
    finished: bool,
}

impl PlaybackHandler {
    fn new(reader: AudioFileReader) -> Self {
        Self {
            reader,
            current_packet: 0,
            finished: false,
        }
    }
    // This handler assumes `buffer` adheres to several invarients:
    // - Are at least `packets_per_buffer` big
    // - Were allocated with packet descriptions
    // - Belong to `audio_queue`
    fn enqueue_next_buffer(
        &mut self,
        audio_queue: QueueRef,
        buffer: BufferRef,
    ) -> PlaybackResult<bool> {
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
        if self.finished {
            println!("ignoring request to fill buffer");
            return Ok(true);
        }

        unsafe {
            let packets_read = self.reader.fill_buffer(buffer, self.current_packet)?;

            if packets_read == 0 {
                self.finished = true;
                return Ok(true);
            }

            //TODO: Extract
            let status = sys::audio_queue_enqueue_buffer(
                audio_queue,
                buffer,
                // Packet descriptions are supplied via buffer itself
                0,
                ptr::null(),
            );

            if status != 0 {
                return Err(PlaybackError::FailedToEnqueueBuffer(status));
            }

            self.current_packet += packets_read as i64;
            Ok(false)
        }
    }
}

struct BufferConfig {
    buffer_size: u32,
    packets_per_buffer: PacketCount,
    format: sys::AudioStreamBasicDescription,
    is_vbr: bool,
}

impl BufferConfig {
    fn from_audio_file(file: AudioFileID) -> PlaybackResult<Self> {
        // Use
        //  - the theoretical max size of a packet of this format
        //  - some heuristics
        //  - a desired buffer duration (aproximate)
        // to determine
        // - how big each buffer needs to be
        // - how many packet to read each time we fill a buffer

        let format = audio_file_read_basic_description(file)?;
        let max_packet_size = audio_file_read_packet_size_upper_bound(file)?;
        let buffer_size = calculate_buffer_size(&format, max_packet_size);
        let packets_per_buffer: u32 = buffer_size / max_packet_size;
        let is_vbr = format.bytes_per_packet == 0 || format.frames_per_packet == 0;
        println!("Buffer size: {buffer_size} bytes");
        println!("Max packet size: {max_packet_size} bytes");
        println!("Packets per buffer: {packets_per_buffer}");
        println!("\nAudio format:\n{format:#?}");
        Ok(BufferConfig {
            buffer_size,
            packets_per_buffer,
            format,
            is_vbr,
        })
    }

    fn packet_descriptions(&self) -> PacketCount {
        if self.is_vbr {
            self.packets_per_buffer
        } else {
            0
        }
    }
}

struct AudioFileReader {
    playback_file: AudioFileID,
    packets_per_buffer: PacketCount,
    is_vbr: bool,
}

impl AudioFileReader {
    fn new(playback_file: AudioFileID, buffer_config: &BufferConfig) -> Self {
        //TODO: Can we introduce a strategy pattern internally to optimise vbr?
        Self {
            playback_file,
            packets_per_buffer: buffer_config.packets_per_buffer,
            is_vbr: buffer_config.is_vbr,
        }
    }

    fn fill_buffer(
        &self,
        buffer: BufferRef,
        from_packet: PacketPosition,
    ) -> PlaybackResult<PacketCount> {
        unsafe {
            let buffer = &mut *buffer;
            let mut num_bytes = buffer.audio_data_bytes_capacity;
            let mut num_packets = self.packets_per_buffer;
            let packet_descriptions = if self.is_vbr {
                buffer.packet_descriptions
            } else {
                ptr::null_mut()
            };

            let status = sys::audio_file_read_packet_data(
                self.playback_file,
                false, // dont use caching
                &mut num_bytes,
                packet_descriptions,
                from_packet,
                &mut num_packets,
                buffer.audio_data,
            );

            if status != 0 {
                return Err(PlaybackError::FailedToReadFromAudioFile(status));
            }

            buffer.audio_data_byte_size = num_bytes;
            buffer.packet_description_count = if self.is_vbr { num_packets } else { 0 };

            Ok(num_packets)
        }
    }
}

fn audio_file_open(path: String) -> PlaybackResult<AudioFileID> {
    if path.is_empty() {
        return Err(PlaybackError::PathIsEmpty);
    }

    let path = CString::new(path)?;
    let path = path.as_bytes();

    unsafe {
        // Create URL
        let url_ref = sys::cfurl_create_from_filesystem_representation(
            ptr::null(), // Use default allocator
            path.as_ptr(),
            path.len() as isize,
            false, // Not a directory
        );

        // Create file
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
            return Err(PlaybackError::FailedToOpenAudioFile(status));
        }

        let file_id = file_id.assume_init();
        Ok(file_id)
    }
}

fn audio_file_close(file: AudioFileID) -> PlaybackResult<()> {
    unsafe {
        let status = sys::audio_file_close(file);
        if status != 0 {
            return Err(PlaybackError::FailedToCloseAudioFile(status));
        }
    }
    Ok(())
}

fn audio_file_read_properties(
    file: AudioFileID,
) -> PlaybackResult<impl Iterator<Item = (String, String)>> {
    unsafe {
        let info_dict = read_audio_file_property(file, sys::AUDIO_FILE_PROPERTY_INFO_DICTIONARY)?;

        // Extract keys and values
        let count = sys::cfdictionary_get_count(info_dict);
        let mut keys = vec![0 as sys::CFStringRef; count as usize];
        let mut values = vec![0 as sys::CFStringRef; count as usize];

        sys::cfdictionary_get_keys_and_values(
            info_dict,
            keys.as_mut_ptr() as *mut *const c_void,
            values.as_mut_ptr() as *mut *const c_void,
        );

        // Copy into Rust strings
        // Note: We use collect to process each cfstring before releasing the dict
        let converter = |s| cfstring_to_string(s);
        let keys: Vec<String> = keys.into_iter().map(converter).collect();
        let values: Vec<String> = values.into_iter().map(converter).collect();
        let properties = keys.into_iter().zip(values);

        sys::cf_release(info_dict as *const c_void);

        Ok(properties)
    }
}

fn audio_file_read_basic_description(
    file: AudioFileID,
) -> PlaybackResult<sys::AudioStreamBasicDescription> {
    unsafe { read_audio_file_property(file, sys::AUDIO_FILE_PROPERTY_DATA_FORMAT) }
}

fn audio_file_read_packet_size_upper_bound(file: AudioFileID) -> PlaybackResult<u32> {
    unsafe { read_audio_file_property(file, sys::AUDIO_FILE_PROPERTY_PACKET_SIZE_UPPER_BOUND) }
}

fn audio_file_read_magic_cookie(file: AudioFileID) -> PlaybackResult<Option<Vec<u8>>> {
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
        if status == i32::from_be_bytes(*b"pty?") {
            return Ok(None);
        }

        // Some other status is probably an error
        if status != 0 {
            return Err(PlaybackError::FailedToReadFilePropertyInfo(status));
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
            return Err(PlaybackError::FailedToReadFileProperty(status));
        }

        assert!(data_size == cookie_size);
        Ok(Some(cookie_data))
    }
}

fn output_queue_create(
    format: *const sys::AudioStreamBasicDescription,
    user_data: *mut c_void,
) -> PlaybackResult<QueueRef> {
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
            return Err(PlaybackError::FailedToCreateAudioQueue(status));
        }
        let output_queue = output_queue.assume_init();
        Ok(output_queue)
    }
}

fn output_queue_set_magic_cookie(output_queue: QueueRef, cookie: Vec<u8>) -> PlaybackResult<()> {
    unsafe {
        let status = sys::audio_queue_set_property(
            output_queue,
            system::AUDIO_QUEUE_PROPERTY_MAGIC_COOKIE_DATA,
            cookie.as_ptr() as *const c_void,
            cookie.len() as u32,
        );

        if status == 0 {
            Ok(())
        } else {
            Err(PlaybackError::FailedToSetAudioQueueProperty(status))
        }
    }
}

fn output_queue_start(output_queue: QueueRef) -> PlaybackResult<()> {
    unsafe {
        let status = sys::audio_queue_start(output_queue, ptr::null());

        if status == 0 {
            Ok(())
        } else {
            Err(PlaybackError::FailedToStartAudioQueue(status))
        }
    }
}

fn output_queue_stop(output_queue: QueueRef, immediate: bool) -> PlaybackResult<()> {
    unsafe {
        let status = sys::audio_queue_stop(output_queue, immediate);

        if status == 0 {
            Ok(())
        } else {
            Err(PlaybackError::FailedToStopAudioQueue(status))
        }
    }
}

fn output_queue_dispose(output_queue: QueueRef, immediate: bool) -> PlaybackResult<()> {
    unsafe {
        let status = sys::audio_queue_dispose(output_queue, immediate);

        if status == 0 {
            Ok(())
        } else {
            Err(PlaybackError::FailedToDisposeAudioQueue(status))
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
    output_queue: QueueRef,
    buffer_config: &BufferConfig,
) -> PlaybackResult<Vec<BufferRef>> {
    unsafe {
        vec![MaybeUninit::uninit(); BUFFER_COUNT]
            .into_iter()
            //TODO: Can we allocate buffers _without_ packet descriptions if we dont need them?
            .map(|mut buffer_ref| {
                let status = sys::audio_queue_allocate_buffer_with_packet_descriptions(
                    output_queue,
                    buffer_config.buffer_size,
                    buffer_config.packet_descriptions(),
                    buffer_ref.as_mut_ptr(),
                );
                if status == 0 {
                    Ok(buffer_ref.assume_init())
                } else {
                    Err(PlaybackError::FailedToAllocateBuffer(status))
                }
            })
            .collect()
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

// This only works with sized types
unsafe fn read_audio_file_property<T>(
    file_id: sys::AudioFileID,
    property: sys::AudioFilePropertyID,
) -> PlaybackResult<T> {
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
        return Err(PlaybackError::FailedToReadFileProperty(status));
    }

    // audio_file_get_property outputs the number of bytes written to data_size
    // Check to see if this is correct for the given type
    assert!(data_size == mem::size_of::<T>() as u32);

    Ok(data)
}

extern "C" fn handle_buffer(user_data: *mut c_void, audio_queue: QueueRef, buffer: BufferRef) {
    println!("Callback thread id: {:?}", std::thread::current().id());

    unsafe {
        let handler = &mut *(user_data as *mut PlaybackHandler);
        //TODO: Handle spurious errors about enqueing during reset
        // Options:
        // Treat it as the sign to stop early (i.e a rude form of communication)
        // Have the other thread warn us not to try, but this would be a race condition
        // (i.e if it stopped the queue after checking the flag and before enqueing)
        // The other is to actually handle the stopping on this thread...
        // (i.e the other thread asks us to stop)
        // ... but what if we got asked to stop half way through reading a file? Hmm.
        // Do we want this to happen here... or inside the handler...
        // ... and what about synchronous pre-buffering?
        if let Err(error) = handler.enqueue_next_buffer(audio_queue, buffer) {
            //TODO: Handle error properly, send back to main thread somehow?
            println!("oh no: {error}");
        }
    }
}
