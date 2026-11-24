//! mango-speech: everything in Mango Loco that listens or talks.
//!
//! * [`transcribe`]: local speech-to-text with word timestamps (whisper.cpp, linked in).
//! * [`models`]: the catalogue of downloadable whisper models and the downloader.
//! * [`align`]: fitting a user-supplied transcript onto recognised word timings.
//! * [`tts`]: text-to-speech through the system synthesiser.
//! * [`record`]: screen / camera / microphone capture through ffmpeg.

mod proc;
pub mod transcribe;
mod words;

/// Returned (inside `anyhow::Error`) when an operation stops because its `cancel` flag was set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cancelled")
    }
}

impl std::error::Error for Cancelled {}

/// True when `err` is the result of a cancelled operation rather than a failure.
pub fn is_cancelled(err: &anyhow::Error) -> bool {
    err.downcast_ref::<Cancelled>().is_some()
}

pub mod align;
pub mod models;
pub mod record;
pub mod tts;
