//! Boombox implements the overall music listening experiance by bringing
//! together the event system, user interface and audio file player

use std::fmt;
use std::ops::ControlFlow::{self, Break, Continue};

use crate::events::{self, Event, EventError};
use crate::player::{PlaybackContext, PlaybackError, PlaybackVolume};
use crate::ui::{self, TerminalUI, UIError};

const UI_TICK_DURATION_MICROSECONDS: i64 = 33333; // 30FPS

use std::{env, process};

#[derive(Debug)]
pub enum BoomboxError {
    Playback(PlaybackError),
    UI(UIError),
    Event(EventError),
}

impl From<PlaybackError> for BoomboxError {
    fn from(err: PlaybackError) -> BoomboxError {
        BoomboxError::Playback(err)
    }
}

impl From<UIError> for BoomboxError {
    fn from(err: UIError) -> BoomboxError {
        BoomboxError::UI(err)
    }
}

impl From<EventError> for BoomboxError {
    fn from(err: EventError) -> BoomboxError {
        BoomboxError::Event(err)
    }
}

impl fmt::Display for BoomboxError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            BoomboxError::Playback(err) => {
                write!(f, "Problem playing back audio, {err}")
            }
            BoomboxError::UI(err) => {
                write!(f, "Problem in UI, {err}")
            }
            BoomboxError::Event(err) => {
                write!(f, "Problem in event loop, {err}")
            }
        }
    }
}

//TODO: Can we get away without the lifetime?
pub struct Boombox<'a> {
    event_sender: events::Sender,
    event_reader: events::Receiver,
    ui: TerminalUI<'a>,
    volume: PlaybackVolume,
}

impl<'a> Boombox<'a> {
    pub fn initialise() -> Result<Self, BoomboxError> {
        //TODO: Pass in file descriptor to build_event_queue
        //TODO: sender and reader are not that accurate names
        let (event_sender, mut event_reader) = events::build_event_queue()?;
        Ok(Boombox {
            event_sender,
            event_reader,
            ui: TerminalUI::activate()?,
            volume: PlaybackVolume::new(),
        })
    }

    //TODO: Still feels like there should be a way to embed context of file that
    // failed in boombox error... so that we can let boombox pull from a playlist
    pub fn play_file(&mut self, path: &str) -> Result<ControlFlow<()>, BoomboxError> {
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

    pub fn shutdown(self) -> Result<(), BoomboxError> {
        self.event_reader.close()?;
        self.ui.deactivate()?;
        Ok(())
    }
}
