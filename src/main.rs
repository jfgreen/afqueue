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

mod events;
mod player;
mod ui;

use std::fmt;

use events::{Event, EventError};
use player::{PlaybackContext, PlaybackError};
use ui::{TerminalUI, UIError};

//TODO: Disable UI tick whilst paused?
const UI_TICK_DURATION_MICROSECONDS: i64 = 33333; // 30FPS

use std::{env, process};

/// Playback a list of files passed in via command line arguments
fn main() {
    let args = env::args();
    let audio_file_paths = parse_args(args);

    let mut ui = TerminalUI::activate().unwrap_or_else(|err| {
        println!("Failed to activate UI, {err}");
        process::exit(1)
    });

    let playback_result = start(audio_file_paths, &mut ui);

    // We are much more likely to get a playback error than a UI error, so we try
    // and deactive the UI first so playback errors can be printed normally
    ui.deactivate()
        .expect("Deactivating UI should succeed, given it activated OK");

    if let Err(err) = playback_result {
        println!("Failed to playback files, {err}");
        process::exit(1)
    }
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
fn start(paths: impl IntoIterator<Item = String>, ui: &mut TerminalUI) -> Result<(), AfqueueError> {
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
        let mut paused = false;

        //TODO: Persist volume across files

        //TODO: Is there a way of making enabling and disabling the timer using
        // idempotent operations so we dont have to track if we have set it or not?
        event_reader.enable_ui_timer_event(UI_TICK_DURATION_MICROSECONDS)?;
        let mut timer_set = true;

        ui.reset_screen()?;
        ui.display_filename(&path)?;
        ui.display_metadata(&metadata)?;

        player.start_playback()?;

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
                    player.decrement_volume()?;
                }
                Event::VolumeUpKeyPressed => {
                    player.increment_volume()?;
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
    Ok(())
}
