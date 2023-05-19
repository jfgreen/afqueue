//! Afqueue manages playback of a queue of audio files.
//!
//! Built on top of the macOS AudioToolbox framework.

#![feature(extern_types)]

mod ffi {
    pub mod audio_toolbox;
    pub mod core_foundation;
    pub mod ioctl;
    pub mod kqueue;
    pub mod termios;
}

mod boombox;
mod events;
mod player;
mod ui;

use boombox::{Boombox, BoomboxError};

use std::ops::ControlFlow::{self, Break, Continue};
use std::{env, process};

fn main() {
    let args = env::args();
    let audio_file_paths = parse_args(args);

    play_audio_files(audio_file_paths).unwrap_or_else(|err| {
        println!("{err}");
        process::exit(1)
    });
}

/// Parse arguments or print help message if supplied invalid input.
fn parse_args(args: impl IntoIterator<Item = String>) -> impl Iterator<Item = String> {
    let mut args = args.into_iter().peekable();
    let exec = args.next();
    if args.peek().is_none() {
        let exec = exec.as_deref().unwrap_or("afqueue");
        println!("Usage: {exec} [audio-file ...]");
        process::exit(1);
    }
    args
}

fn play_audio_files(paths: impl IntoIterator<Item = String>) -> Result<(), BoomboxError> {
    let mut boombox = Boombox::initialise()?;

    let mut result = Ok(Continue(()));
    let mut paths = paths.into_iter();

    while let (Some(path), Ok(Continue(()))) = (paths.next(), &result) {
        //TODO: Figure out how to capture context in error, or pull this up to top
        // level?
        result = boombox.play_file(&path);
    }

    // We are much more likely to encouter a playback error than a UI error, so we
    // try and deactive the UI first so playback errors can be printed normally
    boombox.shutdown()?;

    // We only want to return the error, not control flow
    result.map(|_| ())
}
