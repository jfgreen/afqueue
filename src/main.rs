//! Afqueue manages playback of a queue of audio files.
//!
//! Built on top of the macOS AudioToolbox framework.

// TODO: Come up with new name for this project

// TODO: Diagram of how the different moving parts interact...

// TODO: How do we make sure this code isnt leaky over time?
// TODO: Use kAudioFilePropertyFormatList to deal with multi format files?
// TODO: Query the files channel layout to handle multi channel files?
// TODO: Check we dont orphan threads from one file to the next.
//
// TODO: Figure out where returning a result is over complicated vs a panic

// TODO: Test with channel count > 2 (figure out how we want to support this)
// It looks like we might be able to read kAudioFilePropertyChannelLayout and
// set kAudioQueueProperty_ChannelLayout. Might be interesting to see
// what happens if we set the queue to mono but give it a stereo file

#![feature(extern_types)]

mod ffi {
    pub mod audio_toolbox;
    pub mod core_foundation;
    pub mod ioctl;
    pub mod kqueue;
    pub mod termios;
}

mod events;
mod player;
mod ui;

use std::fmt;

use events::{Event, EventError};
use player::{AudioFilePlayer, PlaybackError};
use ui::{TerminalUI, UIError};

//TODO: Disable UI tick whilst paused?
const UI_TICK_DURATION_MICROSECONDS: i64 = 33333;

use std::{env, process};

/// Playback a list of files passed in via command line arguments
fn main() {
    let args = env::args();
    let audio_file_paths = parse_args(args);
    start(audio_file_paths).unwrap_or_else(|err| {
        println!("Failed to playback file(s)");
        println!("{err}");
        process::exit(1);
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

#[derive(Debug)]
pub enum AfqueueError {
    Playback(PlaybackError),
    UI(UIError),
    Event(EventError),
}

impl From<PlaybackError> for AfqueueError {
    fn from(err: PlaybackError) -> AfqueueError {
        AfqueueError::Playback(err)
    }
}

impl From<UIError> for AfqueueError {
    fn from(err: UIError) -> AfqueueError {
        AfqueueError::UI(err)
    }
}

impl From<EventError> for AfqueueError {
    fn from(err: EventError) -> AfqueueError {
        AfqueueError::Event(err)
    }
}

impl fmt::Display for AfqueueError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            AfqueueError::Playback(err) => {
                write!(f, "Problem playing back audio, {err}")
            }
            AfqueueError::UI(err) => {
                write!(f, "Problem in UI, {err}")
            }
            AfqueueError::Event(err) => {
                write!(f, "Problem in event loop, {err}")
            }
        }
    }
}

//TODO: Make this a bit less nested
fn start(paths: impl IntoIterator<Item = String>) -> Result<(), AfqueueError> {
    let mut ui = TerminalUI::activate()?;

    //TODO: Pass in file descriptor to build_event_queue
    let (event_sender, mut event_reader) = events::build_event_queue()?;

    event_reader.enable_ui_timer_event(UI_TICK_DURATION_MICROSECONDS)?;

    let mut exit_requested = false;

    for path in paths {
        if exit_requested {
            break;
        }

        let mut player = AudioFilePlayer::initialise(&path, event_sender.clone())?;

        let metadata = player.file_metadata()?;

        ui.reset_screen()?;
        ui.display_metadata(&metadata)?;

        player.start_playback()?;

        'event_loop: loop {
            let event = event_reader.next();

            match event {
                Event::PauseKeyPressed => {
                    player.toggle_paused()?;
                }
                Event::NextTrackKeyPressed => {
                    player.stop()?;
                }
                Event::ExitKeyPressed => {
                    player.stop()?;
                    exit_requested = true;
                }
                Event::AudioQueueStopped => {
                    player.close()?;
                    break 'event_loop;
                }
                Event::UITick => {
                    let meter_channels = player.get_meter_level()?;
                    ui.display_meter(meter_channels)?;
                    ui.flush()?;
                    //TODO: Figure out propper timestep that takes into account time spent updating
                    // UI, and general timer inaccuracy
                    event_reader.enable_ui_timer_event(UI_TICK_DURATION_MICROSECONDS)?;
                }
            }
        }
    }
    event_reader.close()?;

    //TODO: Ensure this get called if playback fails, use drop?
    ui.deactivate()?;
    Ok(())
}
