use std::io::{self, Write};
use std::os::fd::AsRawFd;

use std::mem::MaybeUninit;

use crate::ffi::ioctl::{ioctl, WinSize, TIOCGWINSZ};
use crate::ffi::termios::{self, tcgetattr, tcsetattr, Termios};

// Terminal escape codes
const ESCAPE: &str = "\x1b[";
const AUTOWRAP_ENABLE: &str = "?7h";
const AUTOWRAP_DISABLE: &str = "?7l";
const HIDE_CURSOR: &str = "?25l";
const SHOW_CURSOR: &str = "?25h";
const CLEAR_SCREEN: &str = "2J";
const CLEAR_LINE_REMAINDER: &str = "K";
const MOVE_CURSOR: &str = "H";

const COLOUR_RED: &str = "0;31m";
const COLOUR_GREEN: &str = "0;32m";
const COLOUR_YELLOW: &str = "0;33m";
const COLOUR_RESET: &str = "0m";

const NEW_LINE: &str = "\r\n";

const FILENAME_ROW: usize = 1;
const METER_ROW: usize = 2;
const STATUS_ROW: usize = 6;
const VOLUME_ROW: usize = 7;
const METADATA_ROW: usize = 9;

//TODO: Colourised meter?

pub struct TerminalUI<'a> {
    stdout_fd: i32,
    handle: io::StdoutLock<'a>,
    original_termios: Termios,
    size: WinSize,
}

impl<'a> TerminalUI<'a> {
    pub fn activate() -> io::Result<Self> {
        let stdout = io::stdout();
        //TODO: Use new rust 1.70 feature to assert this is a tty
        let mut handle = stdout.lock();
        let stdout_fd = stdout.as_raw_fd();

        let mut termios = read_current_termios(stdout_fd)?;
        let original_termios = termios;
        enable_raw_mode(&mut termios);

        set_termios(stdout_fd, &termios)?;

        let size = read_term_size(stdout_fd)?;

        write!(handle, "{ESCAPE}{HIDE_CURSOR}")?;
        write!(handle, "{ESCAPE}{AUTOWRAP_DISABLE}")?;

        Ok(TerminalUI {
            stdout_fd,
            handle,
            original_termios,
            size,
        })
    }

    pub fn clear_screen(&mut self) -> io::Result<()> {
        write!(self.handle, "{ESCAPE}{CLEAR_SCREEN}")?;
        Ok(())
    }

    pub fn update_size(&mut self) -> io::Result<()> {
        self.size = read_term_size(self.stdout_fd)?;
        Ok(())
    }

    pub fn display_filename(&mut self, filename: &str) -> io::Result<()> {
        write!(self.handle, "{ESCAPE}{};1{MOVE_CURSOR}", FILENAME_ROW)?;
        write!(self.handle, "Playing: {}", filename)?;
        Ok(())
    }

    pub fn display_meter(&mut self, levels: &[f32; 2]) -> io::Result<()> {
        write!(self.handle, "{ESCAPE}{};1{MOVE_CURSOR}", METER_ROW)?;

        //TODO: pull into fields
        let max_bar_length = self.size.ws_col as f32;
        let total_cols = self.size.ws_col as usize;
        let green_cols = (total_cols * 70) / 100;
        let amber_cols = (total_cols * 15) / 100;

        for channel_power in levels {
            let bar_length = (max_bar_length * channel_power).round() as usize;
            write!(self.handle, "{NEW_LINE}")?;
            write!(self.handle, "{ESCAPE}{COLOUR_GREEN}")?;
            //TODO: Is there a nicer less branchy way to do this... with maths?
            for n in 0..bar_length {
                if n == green_cols + 1 {
                    write!(self.handle, "{ESCAPE}{COLOUR_YELLOW}")?;
                }
                if n == green_cols + amber_cols + 1 {
                    write!(self.handle, "{ESCAPE}{COLOUR_RED}")?;
                }
                write!(self.handle, "█")?;
            }
            write!(self.handle, "{ESCAPE}{CLEAR_LINE_REMAINDER}")?;
        }
        write!(self.handle, "{ESCAPE}{COLOUR_RESET}")?;

        Ok(())
    }

    pub fn display_playback_state(&mut self, paused: bool) -> io::Result<()> {
        write!(self.handle, "{ESCAPE}{};1{MOVE_CURSOR}", STATUS_ROW)?;
        if paused {
            write!(self.handle, "⏸")?;
        } else {
            write!(self.handle, "⏵")?;
        }
        Ok(())
    }

    pub fn display_playback_progress(
        &mut self,
        playback_time: f64,
        total_duration: f64,
    ) -> io::Result<()> {
        let playback_secs = playback_time % 60.0;
        let playback_mins = (playback_time / 60.0).floor();
        let total_secs = total_duration % 60.0;
        let total_mins = (total_duration / 60.0).floor();

        write!(self.handle, "{ESCAPE}{};3{MOVE_CURSOR}", STATUS_ROW)?;
        write!(self.handle, "{playback_mins:02.0}:{playback_secs:02.0}")?;
        write!(self.handle, " / {total_mins:02.0}:{total_secs:02.0}")?;
        Ok(())
    }

    pub fn display_volume(&mut self, volume: f32) -> io::Result<()> {
        write!(self.handle, "{ESCAPE}{};1{MOVE_CURSOR}", VOLUME_ROW)?;
        let vol_percent = volume * 100.0;
        write!(self.handle, "Volume: {vol_percent}%")?;
        write!(self.handle, "{ESCAPE}{CLEAR_LINE_REMAINDER}")?;
        Ok(())
    }

    pub fn display_metadata(&mut self, metadata: &[(String, String)]) -> io::Result<()> {
        write!(self.handle, "{ESCAPE}{};1{MOVE_CURSOR}", METADATA_ROW)?;
        write!(self.handle, "Properties:")?;
        write!(self.handle, "{NEW_LINE}")?;
        for (k, v) in metadata {
            write!(self.handle, "{k}: {v}")?;
            write!(self.handle, "{NEW_LINE}")?;
        }
        Ok(())
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.handle.flush()?;
        Ok(())
    }

    pub fn deactivate(mut self) -> io::Result<()> {
        set_termios(self.stdout_fd, &self.original_termios)?;
        write!(self.handle, "{ESCAPE}{CLEAR_SCREEN}")?;
        write!(self.handle, "{ESCAPE}1;1{MOVE_CURSOR}")?;
        write!(self.handle, "{ESCAPE}{SHOW_CURSOR}")?;
        write!(self.handle, "{ESCAPE}{AUTOWRAP_ENABLE}")?;
        self.handle.flush()?;
        Ok(())
    }
}

fn read_current_termios(file_descriptor: i32) -> io::Result<Termios> {
    unsafe {
        let mut termios = MaybeUninit::uninit();
        let result = tcgetattr(file_descriptor, termios.as_mut_ptr());
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(termios.assume_init())
    }
}

fn set_termios(file_descriptor: i32, termios: &Termios) -> io::Result<()> {
    unsafe {
        let result = tcsetattr(file_descriptor, termios::TCSAFLUSH, termios);
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

fn read_term_size(file_descriptor: i32) -> io::Result<WinSize> {
    unsafe {
        let mut win_size = MaybeUninit::uninit();

        let result = ioctl(file_descriptor, TIOCGWINSZ, win_size.as_mut_ptr());

        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(win_size.assume_init())
    }
}

fn enable_raw_mode(termios: &mut Termios) {
    //TODO: Set max speed?

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

    // Disabling INPCK, ISTRIP, and enabling CS8 are traditionally part of
    // setting up "raw terminal output". However, this aleady seems to be
    // the case for Terminal.app
}
