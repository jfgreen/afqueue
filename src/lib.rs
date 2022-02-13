//! The afqueue module manages playback of a queue of audio files.
//!
//! Built on top of the macOS AudioToolbox framework.

#![feature(extern_types)]

use std::env;
use std::ffi::{c_void, CString, NulError};
use std::mem::MaybeUninit;
use std::process;
use std::ptr;

mod system;

use system::OSStatus;

/// Enqueue a list of files passed in via command line arguments and begin
/// playback.
pub fn afqueue_start() {
    let args = env::args();
    let audio_file_path = parse_args_or_print_help(args);
    play(audio_file_path);
}

pub enum PlaybackError {
    PathContainsInteriorNull(NulError),
    PathIsEmpty,
    FailedToOpenAudioFile(OSStatus),
    FailedToCloseAudioFile(OSStatus),
}

impl From<NulError> for PlaybackError {
    fn from(err: NulError) -> PlaybackError {
        PlaybackError::PathContainsInteriorNull(err)
    }
}

fn play(path: String) -> Result<(), PlaybackError> {
    //TODO: Make this code more ideomatic with a newtype pattern?
    if path.is_empty() {
        return Err(PlaybackError::PathIsEmpty);
    }

    let path = CString::new(path)?;
    let path = path.as_bytes();

    unsafe {
        let url_ref = system::cfurl_create_from_filesystem_representation(
            ptr::null(),
            path.as_ptr(),
            path.len() as isize,
            false,
        );

        //TODO: Release somehow?
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

        let status = system::audio_file_close(audio_file_id);
        if status != 0 {
            return Err(PlaybackError::FailedToCloseAudioFile(status));
        }
    }

    Ok(())
}

/// Parse arguments or print help message.
/// Currently returns the first argument.
fn parse_args_or_print_help(args: impl IntoIterator<Item = String>) -> String {
    // TODO: Return a list of files
    let mut args = args.into_iter();
    let exec = args.next();
    match (args.next(), args.next()) {
        (Some(arg), None) => arg,
        _ => {
            let exec = exec.as_deref().unwrap_or("afqueue");
            println!("Usage: {exec} audio-file");
            process::exit(1);
        }
    }
}
