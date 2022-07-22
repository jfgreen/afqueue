use afqueue;
use std::{env, process};

/// Playback a list of files passed in via command line arguments
fn main() {
    let args = env::args();
    let audio_file_paths = parse_args(args);
    //TODO: Change to str
    afqueue::start(audio_file_paths).unwrap_or_else(|err| {
        println!("Failed to playback files");
        println!("{}", err);
        process::exit(1);
    });
}

/// Parse arguments or print help message if supplied invalid input.
fn parse_args(args: impl IntoIterator<Item = String>) -> impl Iterator<Item = String> {
    let mut args = args.into_iter().peekable();
    let exec = args.next();
    if args.peek() == None {
        let exec = exec.as_deref().unwrap_or("afqueue");
        println!("Usage: {exec} [audio-file ...]");
        process::exit(1);
    }
    args
}
