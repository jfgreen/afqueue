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
use player::{PlaybackContext, PlaybackError, PlaybackVolume};
use ui::{TerminalUI, UIError};

//TODO: Disable UI tick whilst paused?
const UI_TICK_DURATION_MICROSECONDS: i64 = 33333; // 30FPS

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

    //TODO: Improve how errors after this point are reported..
    // i.e stop UI, then print

    //TODO: Pass in file descriptor to build_event_queue
    //TODO: sender and reader are not that accurate names
    let (event_sender, mut event_reader) = events::build_event_queue()?;

    let mut exit_requested = false;

    for path in paths {
        if exit_requested {
            break;
        }

        let context = PlaybackContext::new(&path)?;
        let metadata = context.file_metadata()?;
        let mut meter_state = context.new_meter_state();
        let mut handler = context.new_audio_callback_handler(event_sender.clone());
        let mut player = context.new_audio_player(&mut handler)?;
        let mut volume = PlaybackVolume::new();
        let mut paused = false;

        //TODO: Is there a way of making enabling and disabling the timer using
        // idempotent operations so we dont have to track if we have set it or not?
        event_reader.enable_ui_timer_event(UI_TICK_DURATION_MICROSECONDS)?;
        let mut timer_set = true;

        ui.reset_screen()?;
        ui.display_filename(&path)?;
        ui.display_metadata(&metadata)?;

        player.start_playback()?;
        player.set_volume(&volume)?;

        'event_loop: loop {
            let event = event_reader.next();

            match event {
                Event::PauseKeyPressed => {
                    if paused {
                        player.resume()?;
                        //TODO: Minus time since last tick?
                        event_reader.enable_ui_timer_event(UI_TICK_DURATION_MICROSECONDS)?;
                        timer_set = true;
                    } else {
                        player.pause()?;
                        event_reader.disable_ui_timer_event()?;
                        timer_set = false;
                    }
                    paused = !paused;
                }
                Event::VolumeDownKeyPressed => {
                    volume.decrement();
                    player.set_volume(&volume)?;
                }
                Event::VolumeUpKeyPressed => {
                    volume.increment();
                    player.set_volume(&volume)?;
                }
                Event::NextTrackKeyPressed => {
                    player.stop()?;
                }
                Event::ExitKeyPressed => {
                    player.stop()?;
                    exit_requested = true;
                }
                Event::AudioQueueStopped => {
                    //TODO: Is event_reader that accurate a name?
                    if timer_set {
                        event_reader.disable_ui_timer_event()?;
                    }
                    break 'event_loop;
                }
                Event::UITick => {
                    // NOTE: This UI tick event might happen in between a user requested
                    // player.stop() being invoked and the queue actually stopping
                    // (i.e an AudioQueueStopped event)
                    // We are going to assume that this wont cause a problem.

                    player.get_meter_level(&mut meter_state)?;
                    ui.display_meter(meter_state.levels())?;
                    ui.flush()?;
                    //TODO: Figure out propper timestep that takes into account time spent updating
                    // UI, and general timer inaccuracy
                    event_reader.enable_ui_timer_event(UI_TICK_DURATION_MICROSECONDS)?;
                    timer_set = true;
                }
                Event::TerminalResized => {
                    //TODO: Make UI hold on to current metadata/state, resize current bar
                    ui.update_size()?;
                    ui.reset_screen()?;
                    ui.display_filename(&path)?;
                    ui.display_metadata(&metadata)?;
                }
            }
        }
    }
    event_reader.close()?;

    //TODO: Ensure this get called if playback fails, use drop?
    ui.deactivate()?;
    Ok(())
}
