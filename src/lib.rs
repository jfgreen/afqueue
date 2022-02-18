//! The afqueue module manages playback of a queue of audio files.
//!
//! Built on top of the macOS AudioToolbox framework.

#![feature(extern_types)]

use std::ffi::{c_void, CString, NulError};
use std::fmt;
use std::mem::MaybeUninit;
use std::ptr;

mod system;

use system::OSStatus;

pub enum PlaybackError {
    PathContainsInteriorNull(NulError),
    PathIsEmpty,
    FailedToOpenAudioFile(OSStatus),
    FailedToCloseAudioFile(OSStatus),
    FailedToReadFilePropertyInfo(OSStatus),
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
            PlaybackError::PathIsEmpty => write!(f, "Tried to interpret an empty string as a path"),
            PlaybackError::FailedToOpenAudioFile(status) => {
                write!(f, "Failed to open audio file, OSStatus: {}", status)
            }
            PlaybackError::FailedToCloseAudioFile(status) => {
                write!(f, "Failed to close audio file, OSStatus: {}", status)
            }
            PlaybackError::FailedToReadFilePropertyInfo(status) => {
                write!(f, "Failed to read file property info, OSStatus: {}", status)
            }
        }
    }
}

pub fn play(path: String) -> Result<(), PlaybackError> {
    //TODO: Make this code more ideomatic with a newtype pattern?
    if path.is_empty() {
        return Err(PlaybackError::PathIsEmpty);
    }

    let path = CString::new(path)?;
    let path = path.as_bytes();

    unsafe {
        // Create URL
        let url_ref = system::cfurl_create_from_filesystem_representation(
            ptr::null(),
            path.as_ptr(),
            path.len() as isize,
            false,
        );

        // Create file
        let mut audio_file_id = MaybeUninit::uninit();

        let status = system::audio_file_open_url(
            url_ref,
            system::AUDIO_FILE_READ_PERMISSION,
            0, // No file hints
            audio_file_id.as_mut_ptr(),
        );

        //TODO: Make this work on drop?
        system::cf_release(url_ref as *const c_void);

        if status != 0 {
            return Err(PlaybackError::FailedToOpenAudioFile(status));
        }
        let mut audio_file_id = audio_file_id.assume_init();

        let mut info_prop_size = MaybeUninit::uninit();

        // Read file properties
        //TODO: Do we need this - will the property info dictionary always be 8 bytes?
        let status = system::audio_file_get_property_info(
            audio_file_id,
            system::AUDIO_FILE_PROPERTY_INFO_DICTIONARY,
            info_prop_size.as_mut_ptr(),
            ptr::null_mut(),
        );

        if status != 0 {
            return Err(PlaybackError::FailedToReadFilePropertyInfo(status));
        }

        let info_prop_size = info_prop_size.assume_init();

        let status = system::audio_file_close(audio_file_id);
        if status != 0 {
            return Err(PlaybackError::FailedToCloseAudioFile(status));
        }

        println!("{}", info_prop_size);
    }

    Ok(())
}
