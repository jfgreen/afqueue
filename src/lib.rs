//! The afqueue module manages playback of a queue of audio files.
//!
//! Built on top of the macOS AudioToolbox framework.

// TODO: Diagram of how the different moving parts interact...

// TODO: How do we make sure this code isnt leaky over time?
// TODO: Use kAudioFilePropertyFormatList to deal with multi format files?
// TODO: Query the files channel layout to handle multi channel files?
// TODO: Check we dont orphan threads from one file to the next.
// TODO: Start consolidate things into abstractions

#![feature(extern_types)]

//TODO: Is there any point to having a separate main/lib?
//How do we want module visability to work?

mod ffi {
    pub mod audio_toolbox;
    pub mod core_foundation;
    pub mod kqueue;
    pub mod termios;
}

mod events;
mod player;

use std::io::{self, Write};
use std::mem::MaybeUninit;

use events::Event;
use player::{AudioFilePlayer, PlaybackResult};

use ffi::kqueue as kq;
use ffi::termios::{self, tcgetattr, tcsetattr, Termios};

//TODO: Fix metering breaking on terminal resize

//TODO: Disable UI tick whilst paused
const UI_TICK_DURATION_MICROSECONDS: i64 = 33333;

//TODO: Have a new higher level error that wraps each subsystem (player, UI,
// event handler)
pub fn start(paths: impl IntoIterator<Item = String>) -> PlaybackResult<()> {
    let mut termios = read_current_termios()?;
    let original_termios = termios;
    enable_raw_mode(&mut termios);

    set_termios(&termios)?;
    print!("\x1b[?25l"); // Hide cursor

    let (event_kqueue, mut event_reader) = events::build_event_kqueue()?;
    //TODO: Instead of passing around a queue reference (and lots of public
    // methods), consider creating a trivially cloneable event_writer

    events::enable_ui_timer_event(event_kqueue, UI_TICK_DURATION_MICROSECONDS);

    // TODO: This might be nicer / less nested if we pulled from an iterator
    for path in paths {
        print!("\x1b[2J"); // Clear screen
        print!("\x1b[1;1H"); // Position cursor at the top left

        let mut player = AudioFilePlayer::initialise(&path)?;
        //TODO: Experiment with writing explocity to std out via std::io::stdout
        //TODO: Pull out UI stuff into a UI centric component
        print!("Properties:\r\n");
        for (k, v) in player.file_metadata()? {
            print!("{k}: {v}\r\n");
        }

        //TODO: Test with channel count > 2 (figure out how we want to support this)
        // It looks like we might be able to read kAudioFilePropertyChannelLayout and
        // set kAudioQueueProperty_ChannelLayout. Might be interesting to see
        // what happens if we set the queue to mono but give it a stereo file

        player.start_playback(event_kqueue)?;

        //TODO: Make 'q' exit completely, and maybe 's' for skip
        'event_loop: loop {
            let event = event_reader.next();

            match event {
                Event::PauseKeyPressed => {
                    player.toggle_paused()?;
                }
                Event::ExitKeyPressed => {
                    player.stop()?;
                }
                //TODO: Rename to "end of playback" or something
                Event::AudioQueueStopped => {
                    player.close()?;
                    break 'event_loop;
                }
                Event::UITick => {
                    //TODO: Explore different meters
                    //TODO: Maintain a lock on stdout?

                    let meters = player.get_meter_level()?;

                    //TODO: Test UI works when going from mono file to stereo file

                    //TODO: Get max bar length from term
                    let max_bar_length: f32 = 100.0;

                    for channel_power in meters.iter() {
                        print!("\r\n");
                        let bar_length = (max_bar_length * channel_power) as usize;
                        for _ in 0..bar_length {
                            print!("█")
                        }
                        print!("\x1b[K"); // Clear remainder of line
                    }
                    print!("\x1b[{}A", meters.len()); // Hop back up to channel

                    //TODO: Dont ignore this error
                    io::stdout().flush().unwrap();

                    events::enable_ui_timer_event(event_kqueue, UI_TICK_DURATION_MICROSECONDS);
                }
            }
        }
        // Reset the playback finished event for re-use if there is another file to play
        events::enable_playback_finished_event(event_kqueue)?;
    }
    events::close_kqueue(event_kqueue)?;

    set_termios(&original_termios)?;

    //TODO: Ensure these get called if the above has errors
    print!("\x1b[2J"); // Clear screen
    print!("\x1b[1;1H"); // Position cursor at the top left
    print!("\x1b[?25h"); // Show cursor
    Ok(())
}

fn enable_raw_mode(termios: &mut Termios) {
    // Disable echoing
    termios.c_lflag &= !termios::ECHO;

    // Read input byte by byte instead of line by line
    termios.c_lflag &= !termios::ICANON;

    // Disable Ctrl-C and Ctrl-Z signals
    termios.c_lflag &= !termios::ISIG;

    // Disable Ctrl-S and Ctrl-Q flow control
    termios.c_iflag &= !termios::IXON;

    // Disable Ctrl-V (literal quoting) and Ctrl-O (discard pending)
    termios.c_lflag &= !termios::IEXTEN;

    // Fix Ctrl-M by disabling translation of carriage return to newlines
    termios.c_iflag &= !termios::ICRNL;

    // Disable adding a carriage raturn to each outputed newline
    termios.c_oflag &= !termios::OPOST;

    // Disable break condition causing sigint
    termios.c_iflag &= !termios::BRKINT;

    // NOTE: disabling INPCK, ISTRIP, and enabling CS8
    // are traditionally part of setting up "raw terminal output".
    // However, this aleady seems to be the case for Terminal.app
}

fn read_current_termios() -> Result<Termios, io::Error> {
    unsafe {
        let mut termios = MaybeUninit::uninit();
        //TODO: Should STDIN_FILE_NUM be in kqueue
        let result = tcgetattr(kq::STDIN_FILE_NUM as i32, termios.as_mut_ptr());
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(termios.assume_init())
    }
}

fn set_termios(termios: &Termios) -> Result<(), io::Error> {
    unsafe {
        let result = tcsetattr(kq::STDIN_FILE_NUM as i32, termios::TCSAFLUSH, termios);
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}
