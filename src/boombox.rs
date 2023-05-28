//! Boombox implements the overall music listening experiance by bringing
//! together the event system, user interface and audio file player

use std::ops::ControlFlow::{self, Break, Continue};

use crate::error::{AfqueueError, ErrorContext, ErrorCtx};
use crate::events::{self, Event, EventQueue};
use crate::player::{PlaybackContext, PlaybackVolume};
use crate::ui::TerminalUI;

const UI_TICK_DURATION_MICROSECONDS: i64 = 33333; // 30FPS

//TODO: Figure out what error context is useful to add to the below

//TODO: Can we get away without the lifetime?
pub struct Boombox<'a> {
    queue: EventQueue,
    ui: TerminalUI<'a>,
    volume: PlaybackVolume,
}

impl<'a> Boombox<'a> {
    pub fn initialise() -> Result<Self, AfqueueError> {
        //TODO: Pass in file descriptor to build_event_queue
        let queue = events::build_event_queue()?;
        Ok(Boombox {
            queue,
            ui: TerminalUI::activate()?,
            volume: PlaybackVolume::new(),
        })
    }

    //TODO: Might it be nicer for boombox to pull from a playlist?
    pub fn play_file(&mut self, path: &str) -> Result<ControlFlow<()>, AfqueueError> {
        self.play(path)
            .with(ErrorCtx::PlayingBack(path.to_string()))
    }

    fn play(&mut self, path: &str) -> Result<ControlFlow<()>, AfqueueError> {
        let context = PlaybackContext::new(path)?;
        let metadata = context.file_metadata()?;
        let mut meter_state = context.new_meter_state();
        let notifier = self.queue.create_callback_notifier();
        let mut handler = context.new_audio_callback_handler(notifier);
        let mut player = context.new_audio_player(&mut handler)?;

        //TODO: Is there a way of making enabling and disabling the timer using
        // idempotent operations so we dont have to track if we have set it or not?
        self.queue
            .enable_ui_timer_event(UI_TICK_DURATION_MICROSECONDS)?;
        let mut timer_set = true;
        let mut exit_requested = false;
        let mut paused = false;

        self.ui.update_layout(meter_state.channel_count());
        self.ui.clear_screen()?;
        self.ui.display_filename(path)?;
        self.ui.display_meter(meter_state.levels())?;
        self.ui.display_playback_state(paused)?;
        self.ui.display_volume(self.volume.gain())?;
        self.ui.display_metadata(&metadata)?;
        self.ui.flush()?;

        player.set_volume(&self.volume)?;
        player.start_playback()?;

        'event_loop: loop {
            let event = self.queue.next_event();

            match event {
                Event::PauseKeyPressed => {
                    if paused {
                        player.resume()?;
                        //TODO: Minus time since last tick?
                        self.queue
                            .enable_ui_timer_event(UI_TICK_DURATION_MICROSECONDS)?;
                        timer_set = true;
                    } else {
                        player.pause()?;
                        self.queue.disable_ui_timer_event()?;
                        timer_set = false;
                    }
                    paused = !paused;
                    self.ui.display_playback_state(paused)?;
                    self.ui.flush()?;
                }
                Event::VolumeDownKeyPressed => {
                    self.volume.decrement();
                    player.set_volume(&self.volume)?;
                    self.ui.display_volume(self.volume.gain())?;
                    self.ui.flush()?;
                }
                Event::VolumeUpKeyPressed => {
                    self.volume.increment();
                    player.set_volume(&self.volume)?;
                    self.ui.display_volume(self.volume.gain())?;
                    self.ui.flush()?;
                }
                Event::NextTrackKeyPressed => {
                    player.stop()?;
                }
                Event::ExitKeyPressed => {
                    player.stop()?;
                    exit_requested = true;
                }
                Event::PlaybackFinished => {
                    if timer_set {
                        self.queue.disable_ui_timer_event()?;
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
                    self.queue
                        .enable_ui_timer_event(UI_TICK_DURATION_MICROSECONDS)?;
                    timer_set = true;
                }
                Event::TerminalResized => {
                    self.ui.update_size()?;
                    self.ui.clear_screen()?;
                    self.ui.display_filename(path)?;
                    self.ui.display_meter(meter_state.levels())?;
                    self.ui.display_playback_state(paused)?;
                    self.ui.display_volume(self.volume.gain())?;
                    self.ui.display_metadata(&metadata)?;
                    self.ui.flush()?;
                }
            }
        }
        if exit_requested {
            Ok(Break(()))
        } else {
            Ok(Continue(()))
        }
    }

    pub fn shutdown(self) -> Result<(), AfqueueError> {
        self.queue.close()?;
        self.ui.deactivate()?;
        Ok(())
    }
}
