//! Mango Loco media layer.
//!
//! Everything that touches real media lives here: locating and running
//! ffmpeg/ffprobe, probing files into [`mango_core::MediaAsset`]s, waveform
//! peaks, still frames, the text/shape/caption rasteriser shared by preview
//! and export, and the export runner that executes the plans built by
//! [`mango_core::render`].

pub mod export;
pub mod ffmpeg;
pub mod frames;
pub mod probe;
pub mod raster;
pub mod waveform;

pub use ffmpeg::{Cancelled, is_cancelled};
pub use frames::Frame;
pub use waveform::Peaks;
