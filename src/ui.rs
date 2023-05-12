use std::fmt;
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
const MOVE_CURSOR_UPPER_LEFT: &str = ";f";
const MOVE_CURSOR_UP_LINES: &str = "A";

const NEW_LINE: &str = "\r\n";

//TODO: Colourised meter?

#[derive(Debug)]
pub enum UIError {
    IO(io::Error),
}

impl From<io::Error> for UIError {
    fn from(err: io::Error) -> UIError {
        UIError::IO(err)
    }
}

impl fmt::Display for UIError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            UIError::IO(err) => {
                write!(f, "IO error '{err}'")
            }
        }
    }
}

type UIResult = Result<(), UIError>;

pub struct TerminalUI<'a> {
    stdout_fd: i32,
    handle: io::StdoutLock<'a>,
    original_termios: Termios,
    size: WinSize,
}

impl<'a> TerminalUI<'a> {
    pub fn activate() -> Result<Self, UIError> {
        let stdout = io::stdout();
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

    pub fn reset_screen(&mut self) -> UIResult {
        write!(self.handle, "{ESCAPE}{CLEAR_SCREEN}")?;
        write!(self.handle, "{ESCAPE}{MOVE_CURSOR_UPPER_LEFT}")?;
        Ok(())
    }

    pub fn update_size(&mut self) -> UIResult {
        self.size = read_term_size(self.stdout_fd)?;
        Ok(())
    }

    pub fn display_filename(&mut self, filename: &str) -> UIResult {
        write!(self.handle, "Playing: {}", filename)?;
        write!(self.handle, "{NEW_LINE}{NEW_LINE}")?;
        Ok(())
    }

    pub fn display_metadata(&mut self, metadata: &[(String, String)]) -> UIResult {
        write!(self.handle, "Properties:")?;
        write!(self.handle, "{NEW_LINE}")?;
        for (k, v) in metadata {
            write!(self.handle, "{k}: {v}")?;
            write!(self.handle, "{NEW_LINE}")?;
        }
        Ok(())
    }

    pub fn display_meter(&mut self, meter_channels: impl IntoIterator<Item = f32>) -> UIResult {
        let max_bar_length = self.size.ws_col as f32;

        let mut channel_count = 0;
        for channel_power in meter_channels {
            channel_count += 1;
            let bar_length = (max_bar_length * channel_power) as usize;
            write!(self.handle, "{NEW_LINE}")?;
            for _ in 0..bar_length {
                write!(self.handle, "█")?;
            }
            write!(self.handle, "{ESCAPE}{CLEAR_LINE_REMAINDER}")?;
        }

        write!(self.handle, "{ESCAPE}{channel_count}{MOVE_CURSOR_UP_LINES}")?;
        Ok(())
    }

    pub fn flush(&mut self) -> UIResult {
        self.handle.flush()?;
        Ok(())
    }

    pub fn deactivate(mut self) -> UIResult {
        set_termios(self.stdout_fd, &self.original_termios)?;
        write!(self.handle, "{ESCAPE}{CLEAR_SCREEN}")?;
        write!(self.handle, "{ESCAPE}{MOVE_CURSOR_UPPER_LEFT}")?;
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
