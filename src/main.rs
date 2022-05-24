use afqueue::{play, PlaybackError};
use std::{env, process};

/// Playback a list of files passed in via command line arguments
fn main() {
    let args = env::args();
    let audio_file_path = parse_args(args);
    playback_file(&audio_file_path);
}

fn playback_file(path: &str) {
    play(path).unwrap_or_else(|err| {
        println!("Failed to playback files");
        println!("{}", err);
        process::exit(1);
    });
}

/// Parse arguments or print help message.
/// Currently returns the first argument.
fn parse_args(args: impl IntoIterator<Item = String>) -> String {
    // TODO: Return a list of files
    let mut args = args.into_iter();
    let exec = args.next();
    match (args.next(), args.next()) {
        (Some(arg), None) => arg,
        _ => {
            let exec = exec.as_deref().unwrap_or("afqueue");
            println!("Usage: {exec} audio-file");
            process::exit(1);
        }
    }
}
