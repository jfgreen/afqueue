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
use std::ops::ControlFlow::{self, Break, Continue};

use events::{Event, EventError};
use player::{PlaybackContext, PlaybackError, PlaybackVolume};
use ui::{TerminalUI, UIError};

const UI_TICK_DURATION_MICROSECONDS: i64 = 33333; // 30FPS

use std::{env, process};

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

fn play_audio_files(paths: impl IntoIterator<Item = String>) -> Result<(), AfqueueError> {
    let mut afqueue = Afqueue::initialise()?;

    let mut result = Ok(Continue(()));
    let mut paths = paths.into_iter();

    while let (Some(path), Ok(Continue(()))) = (paths.next(), &result) {
        //TODO: Figure out how to capture context in error, or pull this up to top
        // level?
        result = afqueue.play_file(&path);
    }

    // We are much more likely to encouter a playback error than a UI error, so we
    // try and deactive the UI first so playback errors can be printed normally
    afqueue.shutdown()?;

    // We only want to return the error, not control flow
    result.map(|_| ())
}

//TODO: Can we get away without the lifetime?
struct Afqueue<'a> {
    event_sender: events::Sender,
    event_reader: events::Receiver,
    ui: TerminalUI<'a>,
    volume: PlaybackVolume,
}

impl<'a> Afqueue<'a> {
    fn initialise() -> Result<Self, AfqueueError> {
        //TODO: Pass in file descriptor to build_event_queue
        //TODO: sender and reader are not that accurate names
        let (event_sender, mut event_reader) = events::build_event_queue()?;
        Ok(Afqueue {
            event_sender,
            event_reader,
            ui: TerminalUI::activate()?,
            volume: PlaybackVolume::new(),
        })
    }

    fn play_file(&mut self, path: &str) -> Result<ControlFlow<()>, AfqueueError> {
        let context = PlaybackContext::new(&path)?;
        let metadata = context.file_metadata()?;
        let mut meter_state = context.new_meter_state();
        let mut handler = context.new_audio_callback_handler(self.event_sender.clone());
        let mut player = context.new_audio_player(&mut handler)?;

        //TODO: Is there a way of making enabling and disabling the timer using
        // idempotent operations so we dont have to track if we have set it or not?
        self.event_reader
            .enable_ui_timer_event(UI_TICK_DURATION_MICROSECONDS)?;
        let mut timer_set = true;
        let mut exit_requested = false;
        let mut paused = false;

        self.ui.reset_screen()?;
        self.ui.display_filename(&path)?;
        self.ui.display_metadata(&metadata)?;

        player.set_volume(&self.volume)?;
        player.start_playback()?;

        'event_loop: loop {
            let event = self.event_reader.next();

            match event {
                Event::PauseKeyPressed => {
                    if paused {
                        player.resume()?;
                        //TODO: Minus time since last tick?
                        self.event_reader
                            .enable_ui_timer_event(UI_TICK_DURATION_MICROSECONDS)?;
                        timer_set = true;
                    } else {
                        player.pause()?;
                        self.event_reader.disable_ui_timer_event()?;
                        timer_set = false;
                    }
                    paused = !paused;
                }
                Event::VolumeDownKeyPressed => {
                    self.volume.decrement();
                    player.set_volume(&self.volume)?;
                }
                Event::VolumeUpKeyPressed => {
                    self.volume.increment();
                    player.set_volume(&self.volume)?;
                }
                Event::NextTrackKeyPressed => {
                    player.stop()?;
                }
                Event::ExitKeyPressed => {
                    player.stop()?;
                    exit_requested = true;
                }
                Event::PlaybackFinished => {
                    //TODO: Is event_reader that accurate a name?
                    if timer_set {
                        self.event_reader.disable_ui_timer_event()?;
                    }
                    break 'event_loop;
                }
                Event::UITick => {
                    // NOTE: This UI tick event might happen in between a user requested
                    // player.stop() being invoked and the queue actually stopping
                    // (i.e an AudioQueueStopped event)
                    // We are going to assume that this wont cause a problem.

                    player.get_meter_level(&mut meter_state)?;
                    self.ui.display_meter(meter_state.levels())?;
                    self.ui.flush()?;
                    //TODO: Figure out propper timestep that takes into account time spent updating
                    // UI, and general timer inaccuracy
                    self.event_reader
                        .enable_ui_timer_event(UI_TICK_DURATION_MICROSECONDS)?;
                    timer_set = true;
                }
                Event::TerminalResized => {
                    //TODO: Make UI hold on to current metadata/state, resize current bar
                    self.ui.update_size()?;
                    self.ui.reset_screen()?;
                    self.ui.display_filename(&path)?;
                    self.ui.display_metadata(&metadata)?;
                }
            }
        }
        if exit_requested {
            Ok(Break(()))
        } else {
            Ok(Continue(()))
        }
    }

    fn shutdown(self) -> Result<(), AfqueueError> {
        self.event_reader.close()?;
        self.ui.deactivate()?;
        Ok(())
    }
}
