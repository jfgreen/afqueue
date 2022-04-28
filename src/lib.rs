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

pub enum PlaybackError {
    PathContainsInteriorNull(NulError),
    PathIsEmpty,
    FailedToOpenAudioFile(sys::OSStatus),
    FailedToCloseAudioFile(sys::OSStatus),
    FailedToReadFileProperty(sys::OSStatus),
    FailedToReadFilePropertyInfo(sys::OSStatus),
    FailedToCreateAudioQueue(sys::OSStatus),
    FailedToSetAudioQueueProperty(sys::OSStatus),
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
        }
    }
}

//struct PlaybackState {
//playing_file, sys::AudioFileID,
//current_packet: u64, // TODO: Does this have to be i64?
//bytes_to_read: u32,
//packets_to_read: u32,
//packet_descriptions: *const AudioStreamPacketDescription,
//boolean: finished,
//}

struct AudioFile {
    file_id: sys::AudioFileID,
}

impl AudioFile {
    fn open(path: String) -> Result<AudioFile, PlaybackError> {
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
            Ok(AudioFile { file_id })
        }
    }

    fn read_properties(&self) -> Result<impl Iterator<Item = (String, String)>, PlaybackError> {
        unsafe {
            let info_dict =
                read_audio_file_property(self.file_id, sys::AUDIO_FILE_PROPERTY_INFO_DICTIONARY)?;

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

    fn read_basic_description(&self) -> Result<sys::AudioStreamBasicDescription, PlaybackError> {
        unsafe { read_audio_file_property(self.file_id, sys::AUDIO_FILE_PROPERTY_DATA_FORMAT) }
    }

    fn read_packet_size_upper_bound(&self) -> Result<u32, PlaybackError> {
        unsafe {
            read_audio_file_property(
                self.file_id,
                sys::AUDIO_FILE_PROPERTY_PACKET_SIZE_UPPER_BOUND,
            )
        }
    }

    fn read_magic_cookie(&self) -> Result<Option<Vec<u8>>, PlaybackError> {
        unsafe {
            // Check to see if there is a cookie, and if so how large it is.
            let mut cookie_size: u32 = 0;
            let mut is_writable: u32 = 0;
            let status = sys::audio_file_get_property_info(
                self.file_id,
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
                self.file_id,
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

    fn close(self) -> Result<(), PlaybackError> {
        unsafe {
            let status = sys::audio_file_close(self.file_id);
            if status != 0 {
                return Err(PlaybackError::FailedToCloseAudioFile(status));
            }
        }
        Ok(())
    }
}

// TODO: Use kAudioFilePropertyFormatList to deal with multi format files?

// TODO: Query the files channel layout to handle multi channel files?

pub fn play(path: String) -> Result<(), PlaybackError> {
    let audio_file = AudioFile::open(path)?;

    println!("Properties:");
    for (k, v) in audio_file.read_properties()? {
        println!("{k}: {v}");
    }

    let format = audio_file.read_basic_description()?;
    println!("\nAudio format:\n{format:#?}");

    unsafe {
        // Create output audio queue
        let mut output_queue = MaybeUninit::uninit();
        let status = sys::audio_queue_new_output(
            &format,
            output_callback,
            ptr::null_mut() as *mut c_void, // Callback data
            0 as *const _,                  // Run loop
            0 as *const _,                  // Run loop mode
            0,                              // flags
            output_queue.as_mut_ptr(),
        );

        if status != 0 {
            return Err(PlaybackError::FailedToCreateAudioQueue(status));
        }
        let output_queue = output_queue.assume_init();

        // Copy magic cookie data to audio queue
        if let Some(cookie) = audio_file.read_magic_cookie()? {
            println!("Magic cookie is {} bytes", cookie.len());

            let status = sys::audio_queue_set_property(
                output_queue,
                system::AUDIO_QUEUE_PROPERTY_MAGIC_COOKIE_DATA,
                cookie.as_ptr() as *const c_void,
                cookie.len() as u32,
            );

            if status != 0 {
                return Err(PlaybackError::FailedToSetAudioQueueProperty(status));
            }
        }

        // Use
        //  - the theoretical max size of a packet of this format
        //  - some heuristics
        //  - a desired buffer duration (aproximate)
        // to determine
        // - how big each buffer needs to be
        // - how many packet to read each time we fill a buffer

        //TODO: Write some tests for this calculation
        let max_packet_size = audio_file.read_packet_size_upper_bound()?;

        let buffer_size: u32 = if format.frames_per_packet != 0 {
            // If frames per packet are known, tailor the buffer size.
            let frames = format.sample_rate * BUFFER_SECONDS_HINT;
            let packets = (frames / (format.frames_per_packet as f64)).ceil() as u32;
            let size = packets * max_packet_size;
            let size = cmp::max(size, LOWER_BUFFER_SIZE_HINT);
            let size = cmp::min(size, UPPER_BUFFER_SIZE_HINT);
            size
        } else {
            // If frames per packet is not known, fallback to something large enough
            cmp::max(max_packet_size, UPPER_BUFFER_SIZE_HINT)
        };

        println!("buffer_size: {buffer_size} bytes");
        let packets_per_buffer: u32 = buffer_size / max_packet_size;
        println!("packets_per_buffer: {packets_per_buffer}");

        // If format is VBR, allocate memory for packet array.

        let is_vbr = format.bytes_per_packet == 0 || format.frames_per_packet == 0;
        let packet_descs = if is_vbr {
            Some(vec![
                sys::AudioStreamPacketDescription::default();
                packets_per_buffer as usize
            ])
        } else {
            None
        };

        //TODO: Prime queue?
        //TODO: Stop and reset in between files?
    }

    audio_file.close()?;
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

extern "C" fn output_callback(
    user_data: *mut c_void,
    audio_queue: sys::AudioQueueRef,
    buffer: sys::AudioQueueBufferRef,
) {
}
