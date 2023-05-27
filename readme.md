# Afqueue

A macOS TUI app to play back a series of audio files using core audio.

Currently only tested on Terminal.app.

Usage:

```
afqueue somefile.mp3 anotherfile.wav
```

This play nicely with most shells like so:

```
afqueue *.flac
```

Controls:

| Key | Action             |
| --- | ------------------ |
| n   | Skip to next track |
| p   | Toggle paused      |
| ]   | Volume up          |
| [   | Volume down        |
| q   | Exit               |
