use std::fmt;
use std::io::{self, Write};
use std::os::fd::AsRawFd;

use std::mem::MaybeUninit;

use crate::ffi::termios::{self, tcgetattr, tcsetattr, Termios};

// Terminal escape codes
const ESCAPE: &str = "\x1b[";
const HIDE_CURSOR: &str = "?25l";
const SHOW_CURSOR: &str = "?25h";
const CLEAR_SCREEN: &str = "2J";
const CLEAR_LINE_REMAINDER: &str = "K";
const MOVE_CURSOR_UPPER_LEFT: &str = ";f";
const MOVE_CURSOR_UP_LINES: &str = "A";

const NEW_LINE: &str = "\r\n";

//TODO: Fix metering breaking on terminal resize
//TODO: Maintain a lock on stdout?
//TODO: Explore different meters
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
                write!(f, "IO error interacting with termios: '{err}'")
            }
        }
    }
}

type UIResult = Result<(), UIError>;

pub struct TerminalUI {
    stdout: io::Stdout,
    original_termios: Termios,
}

impl TerminalUI {
    pub fn activate() -> Result<Self, UIError> {
        //TODO: try locking stdout
        let mut stdout = io::stdout();

        let mut termios = read_current_termios(&stdout)?;

        let original_termios = termios;
        enable_raw_mode(&mut termios);

        set_termios(&mut stdout, &termios)?;

        write!(stdout, "{ESCAPE}{HIDE_CURSOR}")?;

        Ok(TerminalUI {
            stdout,
            original_termios,
        })
    }

    pub fn reset_screen(&mut self) -> UIResult {
        write!(self.stdout, "{ESCAPE}{CLEAR_SCREEN}")?;
        write!(self.stdout, "{ESCAPE}{MOVE_CURSOR_UPPER_LEFT}")?;
        Ok(())
    }

    pub fn display_metadata(&mut self, metadata: &[(String, String)]) -> UIResult {
        write!(self.stdout, "Properties:{NEW_LINE}")?;
        for (k, v) in metadata {
            write!(self.stdout, "{k}: {v}{NEW_LINE}")?;
        }
        Ok(())
    }

    pub fn display_meter(&mut self, meter_channels: Vec<f32>) -> UIResult {
        //TODO: Get max bar length from term
        let max_bar_length: f32 = 100.0;

        for channel_power in meter_channels.iter() {
            let bar_length = (max_bar_length * channel_power) as usize;
            write!(self.stdout, "{NEW_LINE}")?;
            for _ in 0..bar_length {
                write!(self.stdout, "█")?;
            }
            write!(self.stdout, "{ESCAPE}{CLEAR_LINE_REMAINDER}")?;
        }

        let channel_count = meter_channels.len();
        write!(self.stdout, "{ESCAPE}{channel_count}{MOVE_CURSOR_UP_LINES}")?;
        Ok(())
    }

    pub fn flush(&mut self) -> UIResult {
        self.stdout.flush()?;
        Ok(())
    }

    pub fn deactivate(mut self) -> UIResult {
        set_termios(&mut self.stdout, &self.original_termios)?;
        write!(self.stdout, "{ESCAPE}{CLEAR_SCREEN}")?;
        write!(self.stdout, "{ESCAPE}{MOVE_CURSOR_UPPER_LEFT}")?;
        write!(self.stdout, "{ESCAPE}{SHOW_CURSOR}")?;
        Ok(())
    }
}

fn read_current_termios(stdout: &io::Stdout) -> io::Result<Termios> {
    unsafe {
        let mut termios = MaybeUninit::uninit();
        let result = tcgetattr(stdout.as_raw_fd(), termios.as_mut_ptr());
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(termios.assume_init())
    }
}

fn set_termios(stdout: &mut io::Stdout, termios: &Termios) -> io::Result<()> {
    unsafe {
        let result = tcsetattr(stdout.as_raw_fd(), termios::TCSAFLUSH, termios);
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
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
