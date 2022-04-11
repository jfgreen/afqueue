//! The afqueue module manages playback of a queue of audio files.
//!
//! Built on top of the macOS AudioToolbox framework.

#![feature(extern_types)]

use std::ffi::{c_void, CString, NulError};
use std::fmt;
use std::mem::{self, MaybeUninit};
use std::ptr;

mod system;

use system as sys;

pub enum PlaybackError {
    PathContainsInteriorNull(NulError),
    PathIsEmpty,
    FailedToOpenAudioFile(sys::OSStatus),
    FailedToCloseAudioFile(sys::OSStatus),
    FailedToReadFileProperty(sys::OSStatus),
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

    //TODO: Return properties instead
    fn print_properties(&self) -> Result<(), PlaybackError> {
        unsafe {
            let info_dict =
                read_audio_file_property(self.file_id, sys::AUDIO_FILE_PROPERTY_INFO_DICTIONARY)?;

            // Extract keys and values
            let count = sys::cfdictionary_get_count(info_dict);
            let mut keys = Vec::<sys::CFStringRef>::with_capacity(count as usize);
            let mut values = Vec::<sys::CFStringRef>::with_capacity(count as usize);
            sys::cfdictionary_get_keys_and_values(
                info_dict,
                keys.as_mut_ptr() as *mut *const c_void,
                values.as_mut_ptr() as *mut *const c_void,
            );

            keys.set_len(count as usize);
            values.set_len(count as usize);

            // Convert to Rust strings
            println!("keys:");
            for k in keys {
                let s = cfstring_to_string(k);
                println!("{s}");
            }

            println!("\nvalues:");
            for v in values {
                let s = cfstring_to_string(v);
                println!("{s}");
            }

            //TODO: Do we also have to release the contents of the dictionary?
            sys::cf_release(info_dict as *const c_void);

            println!("count: {count}");
        }

        Ok(())
    }

    fn get_basic_description(&self) -> Result<sys::AudioStreamBasicDescription, PlaybackError> {
        unsafe { read_audio_file_property(self.file_id, sys::AUDIO_FILE_PROPERTY_DATA_FORMAT) }
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

pub fn play(path: String) -> Result<(), PlaybackError> {
    let audio_file = AudioFile::open(path)?;
    audio_file.print_properties()?;
    let basic_description = audio_file.get_basic_description()?;
    println!("{basic_description:?}");

    audio_file.close()?;
    Ok(())
}

unsafe fn cfstring_to_string(cfstring: sys::CFStringRef) -> String {
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
