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

use system as sys;

const LOWER_BUFFER_SIZE_HINT: u32 = 0x4000;
const UPPER_BUFFER_SIZE_HINT: u32 = 0x50000;
const BUFFER_SECONDS_HINT: f64 = 0.5;
const BUFFER_COUNT: usize = 3;

pub enum PlaybackError {
    PathContainsInteriorNull(NulError),
    PathIsEmpty,
    FailedToOpenAudioFile(sys::OSStatus),
    FailedToCloseAudioFile(sys::OSStatus),
    FailedToReadFileProperty(sys::OSStatus),
    FailedToReadFilePropertyInfo(sys::OSStatus),
    FailedToCreateAudioQueue(sys::OSStatus),
    FailedToSetAudioQueueProperty(sys::OSStatus),
    FailedToAllocateBuffer(sys::OSStatus),
    FailedToStartAudioQueue(sys::OSStatus),
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
            PlaybackError::FailedToOpenAudioFile(status) => {
                write!(f, "Failed to open audio file, OSStatus: {}", status)
            }
            PlaybackError::FailedToCloseAudioFile(status) => {
                write!(f, "Failed to close audio file, OSStatus: {}", status)
            }
            PlaybackError::FailedToReadFileProperty(status) => {
                write!(f, "Failed to read file property, OSStatus: {}", status)
            }
            PlaybackError::FailedToReadFilePropertyInfo(status) => {
                write!(f, "Failed to read file property, OSStatus: {}", status)
            }
            PlaybackError::FailedToCreateAudioQueue(status) => {
                write!(f, "Failed to create audio queue, OSStatus: {}", status)
            }
            PlaybackError::FailedToSetAudioQueueProperty(status) => {
                write!(
                    f,
                    "Failed to set audio queue property, OSStatus: {}",
                    status
                )
            }
            PlaybackError::FailedToAllocateBuffer(status) => {
                write!(f, "Failed to allocate buffer, OSStatus: {}", status)
            }
            PlaybackError::FailedToStartAudioQueue(status) => {
                write!(f, "Failed to start audio queue, OSStatus: {}", status)
            }
        }
    }
}

struct PlaybackState {
    playing_file: sys::AudioFileID,
    packets_per_buffer: u32, //TODO: Can this be stored in buffer instead?
    is_vbr: bool,
    current_packet: i64,
    finished: bool,
}

fn audio_file_open(path: String) -> Result<sys::AudioFileID, PlaybackError> {
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

fn audio_file_read_properties(
    file_id: sys::AudioFileID,
) -> Result<impl Iterator<Item = (String, String)>, PlaybackError> {
    unsafe {
        let info_dict =
            read_audio_file_property(file_id, sys::AUDIO_FILE_PROPERTY_INFO_DICTIONARY)?;

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
        let keys: Vec<String> = keys.into_iter().map(|k| cfstring_to_string(k)).collect();
        let values: Vec<String> = values.into_iter().map(|v| cfstring_to_string(v)).collect();
        let properties = keys.into_iter().zip(values);

        //TODO: Do we also have to release the contents of the dictionary?
        sys::cf_release(info_dict as *const c_void);

        Ok(properties)
    }
}

fn audio_file_read_basic_description(
    file_id: sys::AudioFileID,
) -> Result<sys::AudioStreamBasicDescription, PlaybackError> {
    unsafe { read_audio_file_property(file_id, sys::AUDIO_FILE_PROPERTY_DATA_FORMAT) }
}

fn audio_file_read_packet_size_upper_bound(
    file_id: sys::AudioFileID,
) -> Result<u32, PlaybackError> {
    unsafe { read_audio_file_property(file_id, sys::AUDIO_FILE_PROPERTY_PACKET_SIZE_UPPER_BOUND) }
}

fn audio_file_read_magic_cookie(
    file_id: sys::AudioFileID,
) -> Result<Option<Vec<u8>>, PlaybackError> {
    unsafe {
        // Check to see if there is a cookie, and if so how large it is.
        let mut cookie_size: u32 = 0;
        let mut is_writable: u32 = 0;
        let status = sys::audio_file_get_property_info(
            file_id,
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
            file_id,
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

fn audio_file_close(file_id: sys::AudioFileID) -> Result<(), PlaybackError> {
    unsafe {
        let status = sys::audio_file_close(file_id);
        if status != 0 {
            return Err(PlaybackError::FailedToCloseAudioFile(status));
        }
    }
    Ok(())
}

//TODO: Pass in callback
fn output_queue_create(
    format: &sys::AudioStreamBasicDescription,
    //TODO: Consider using a propper type
    user_data: *mut c_void,
) -> Result<sys::AudioQueueRef, PlaybackError> {
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

fn output_queue_set_magic_cookie(
    output_queue: sys::AudioQueueRef,
    cookie: Vec<u8>,
) -> Result<(), PlaybackError> {
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

// TODO: Use kAudioFilePropertyFormatList to deal with multi format files?
// TODO: Query the files channel layout to handle multi channel files?

pub fn play(path: String) -> Result<(), PlaybackError> {
    let playback_file = audio_file_open(path)?;

    println!("Properties:");
    for (k, v) in audio_file_read_properties(playback_file)? {
        println!("{k}: {v}");
    }

    let format = audio_file_read_basic_description(playback_file)?;
    println!("\nAudio format:\n{format:#?}");

    unsafe {
        // Use
        //  - the theoretical max size of a packet of this format
        //  - some heuristics
        //  - a desired buffer duration (aproximate)
        // to determine
        // - how big each buffer needs to be
        // - how many packet to read each time we fill a buffer

        //TODO: Write some tests for this calculation
        let max_packet_size = audio_file_read_packet_size_upper_bound(playback_file)?;

        let buffer_size: u32 = if format.frames_per_packet != 0 {
            println!("Known frames per packet");
            // If frames per packet are known, tailor the buffer size.
            let frames = format.sample_rate * BUFFER_SECONDS_HINT;
            let packets = (frames / (format.frames_per_packet as f64)).ceil() as u32;
            let size = packets * max_packet_size;
            let size = cmp::max(size, LOWER_BUFFER_SIZE_HINT);
            let size = cmp::min(size, UPPER_BUFFER_SIZE_HINT);
            size
        } else {
            println!("Unknown frames per packet");
            // If frames per packet is not known, fallback to something large enough
            cmp::max(max_packet_size, UPPER_BUFFER_SIZE_HINT)
        };

        println!("buffer_size: {buffer_size} bytes");
        println!("max_packet_size: {max_packet_size} bytes");
        let packets_per_buffer: u32 = buffer_size / max_packet_size;
        println!("packets_per_buffer: {packets_per_buffer}");

        let is_vbr = format.bytes_per_packet == 0 || format.frames_per_packet == 0;
        let packet_description_count = if is_vbr { packets_per_buffer } else { 0 };

        let mut state = PlaybackState {
            playing_file: playback_file,
            current_packet: 0,
            packets_per_buffer: packets_per_buffer,
            is_vbr: is_vbr,
            finished: false,
        };

        let state_ptr = &mut state as *mut _ as *mut c_void;

        let output_queue = output_queue_create(&format, state_ptr)?;

        if let Some(cookie) = audio_file_read_magic_cookie(playback_file)? {
            println!("Magic cookie is {} bytes", cookie.len());
            output_queue_set_magic_cookie(output_queue, cookie)?;
        }

        //TODO: Double check what these buffer references are needed for
        let buffers = vec![MaybeUninit::uninit(); BUFFER_COUNT]
            .into_iter()
            .map(|mut buffer_ref| {
                let status = sys::audio_queue_allocate_buffer_with_packet_descriptions(
                    output_queue,
                    buffer_size,
                    packet_description_count,
                    buffer_ref.as_mut_ptr(),
                );
                if status == 0 {
                    Ok(buffer_ref.assume_init())
                } else {
                    Err(PlaybackError::FailedToAllocateBuffer(status))
                }
            })
            .collect::<Result<Vec<sys::AudioQueueBufferRef>, PlaybackError>>()?;

        //TODO: Check the below behaves sensibly for small files

        // Pre load buffers with audio
        for buffer in buffers {
            // For small files the entire audio could be less than the buffers
            if state.finished {
                break;
                //TODO: Is there a better way than a break in a for loop
            }
            handle_buffer(state_ptr, output_queue, buffer);
        }

        let status = sys::audio_queue_start(output_queue, ptr::null());

        if status != 0 {
            return Err(PlaybackError::FailedToStartAudioQueue(status));
        }

        //FIXME: Create real controls
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).unwrap();

        //TODO: Stop and dispose of queue
    }

    audio_file_close(playback_file)?;
    Ok(())
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
) -> Result<T, PlaybackError> {
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
    // Check to see if this is correct for our given type
    assert!(data_size == mem::size_of::<T>() as u32);

    return Ok(data);
}

extern "C" fn handle_buffer(
    user_data: *mut c_void,
    audio_queue: sys::AudioQueueRef,
    buffer: sys::AudioQueueBufferRef,
) {
    //TODO: Can we extract the middle out of this to make initial pre-buffering
    // easier?
    unsafe {
        // TODO: exit early if finished is true

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
        let state = user_data as *mut PlaybackState;
        let mut num_bytes = (*buffer).audio_data_bytes_capacity;
        let mut num_packets = (*state).packets_per_buffer;

        //TODO: Can we de-reference up front?

        let status = sys::audio_file_read_packet_data(
            (*state).playing_file,
            false, // dont use caching
            &mut num_bytes,
            if (*state).is_vbr {
                (*buffer).packet_descriptions
            } else {
                ptr::null_mut()
            },
            (*state).current_packet,
            &mut num_packets,
            (*buffer).audio_data,
        );

        //TODO: Do something with read status

        if num_packets > 0 {
            (*buffer).audio_data_byte_size = num_bytes;
            (*buffer).packet_description_count = if (*state).is_vbr { num_packets } else { 0 };
            let status = sys::audio_queue_enqueue_buffer(
                audio_queue,
                buffer,
                // Packet descriptions are supplied via buffer itself
                0,
                ptr::null(),
            );
            //TODO: Do something with enqueue status
            (*state).current_packet += num_packets as i64;
        } else {
            // TODO: Stop queue here?
            (*state).finished = true;
        }
    }
}
