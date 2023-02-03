use std::fmt;
use std::io::{self, Write};

use std::mem::MaybeUninit;

use crate::ffi::termios::{self, tcgetattr, tcsetattr, Termios};

pub struct TerminalUI {
    file_descriptor: i32,
    original_termios: Termios,
}

//TODO: Use constants (or little functions?) for escape codes
//TODO: Replace print! macro with more explicit write to stdout
//TODO: Fix metering breaking on terminal resize
//TODO: Maintain a lock on stdout?

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

impl TerminalUI {
    pub fn activate(file_descriptor: i32) -> Result<Self, UIError> {
        let mut termios = read_current_termios(file_descriptor)?;
        let original_termios = termios;
        enable_raw_mode(&mut termios);

        set_termios(file_descriptor, &termios)?;
        print!("\x1b[?25l"); // Hide cursor

        Ok(TerminalUI {
            file_descriptor,
            original_termios,
        })
    }

    pub fn clear_screen(&self) {
        print!("\x1b[2J"); // Clear screen
        print!("\x1b[1;1H"); // Position cursor at the top left
    }

    pub fn display_metadata(&self, metadata: Vec<(String, String)>) {
        print!("Properties:\r\n");
        for (k, v) in metadata {
            print!("{k}: {v}\r\n");
        }
    }

    pub fn display_meter(&self, meter_channels: Vec<f32>) {
        //TODO: Explore different meters
        //TODO: Colourised meter?

        //TODO: Get max bar length from term
        let max_bar_length: f32 = 100.0;

        for channel_power in meter_channels.iter() {
            print!("\r\n");
            let bar_length = (max_bar_length * channel_power) as usize;
            for _ in 0..bar_length {
                print!("█")
            }
            print!("\x1b[K"); // Clear remainder of line
        }
        print!("\x1b[{}A", meter_channels.len()); // Hop back up to channel
    }

    pub fn flush(&self) {
        //TODO: Dont ignore this error
        io::stdout().flush().unwrap();
    }

    pub fn deactivate(self) -> Result<(), UIError> {
        //TODO: Dont ignore this error
        set_termios(self.file_descriptor, &self.original_termios)?;
        print!("\x1b[2J"); // Clear screen
        print!("\x1b[1;1H"); // Position cursor at the top left
        print!("\x1b[?25h"); // Show cursor
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

fn read_current_termios(file_descriptor: i32) -> Result<Termios, io::Error> {
    unsafe {
        let mut termios = MaybeUninit::uninit();
        let result = tcgetattr(file_descriptor, termios.as_mut_ptr());
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(termios.assume_init())
    }
}

fn set_termios(file_descriptor: i32, termios: &Termios) -> Result<(), io::Error> {
    unsafe {
        let result = tcsetattr(file_descriptor, termios::TCSAFLUSH, termios);
        if result < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}
