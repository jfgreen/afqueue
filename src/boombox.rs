//! Boombox implements the overall music listening experiance by bringing
//! together the event system, user interface and audio file player

use std::ops::ControlFlow::{self, Break, Continue};

use crate::error::{AfqueueError, ErrorContext, ErrorCtx};
use crate::events::{self, Event, EventQueue};
use crate::player::{PlaybackContext, PlaybackVolume};
use crate::ui::TerminalUI;

const UI_TICK_DURATION_MICROSECONDS: i64 = 33333; // 30FPS
const UPDATE_PROGRESS_TICK_FREQUENCY: usize = 30; // Every second

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
        let estimated_duration = context.estimated_duration()?;
        let mut meter_state = context.new_meter_state();
        let notifier = self.queue.create_callback_notifier();
        let mut handler = context.new_audio_callback_handler(notifier);
        let mut player = context.new_audio_player(&mut handler)?;

        let timer_set = true;
        let mut exit_requested = false;
        let mut paused = false;
        let mut tick_count = 0;

        //TODO: Duplicated below
        //TODO: Would it be eaiser to enforce a stereo meter?
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

        // Timer will fire periodically, but wont create duplicates events if we leave
        // it on the queue

        'event_loop: loop {
            let event = self.queue.next_event();

            match event {
                Event::PauseKeyPressed => {
                    //TODO: Might be worth updating playback progress on pause
                    if paused {
                        player.resume()?;
                    } else {
                        player.pause()?;
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
                Event::PlaybackStarted => {
                    self.queue
                        .enable_ui_timer_event(UI_TICK_DURATION_MICROSECONDS)?;
                }
                Event::PlaybackFinished => {
                    break 'event_loop;
                }
                Event::UITick => {
                    // TODO: Ignore (or disable?) UI ticks while paused? OR do a cool paused
                    // animation

                    // Polling the playback progress of the player while it is starting up or
                    // shutting down will trigger an error.
                    //
                    // While it would be nice to avoid scheduling UI ticks to occur during these
                    // times, this turns out to be hard. Even if we try and track its state in this
                    // thread we run into the following problems:
                    //
                    // a) A UI tick might fall in between a user requested stop being invoked and
                    // the player actually stopping
                    //
                    // b) A UI tick might fall in between the player coming to a stop on its own and
                    // us finding out in this thread via the event queue
                    //
                    // c) Any check we do here of the player state might be outdated in between
                    // checking and actioning
                    //
                    // Therefore the only sensible thing to do is to ask for forgiveness instead of
                    // permission! So get_playback_time might not return a value.

                    player.get_meter_level(&mut meter_state)?;
                    self.ui.display_meter(meter_state.levels())?;

                    if tick_count % UPDATE_PROGRESS_TICK_FREQUENCY == 0 {
                        if let Some(progress) = player.get_playback_time()? {
                            self.ui
                                .display_playback_progress(progress, estimated_duration)?;
                        }
                    }
                    self.ui.flush()?;
                    tick_count += 1;
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
        if timer_set {
            self.queue.disable_ui_timer_event()?;
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
