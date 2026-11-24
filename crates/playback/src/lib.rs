//! Live preview engine for Mango Loco.
//!
//! Plays an edited composition straight from the source files, without
//! rendering it first:
//!
//! * [`pcm`]: every source's audio decoded once to a memory-mapped cache;
//! * [`mix`]: the audible timeline of a composition and the reference mixer;
//! * [`audio`]: realtime output of a timeline (the preview's master clock);
//! * [`video`]: decoded video frames for the script track and video layers.
//!
//! Typical use: `PcmCache::ensure` each asset on a worker thread, build a
//! `mix::Timeline` after every edit and hand it to the engine with
//! `set_timeline`, and every UI frame ask `ScriptVideo::request` for the
//! picture at `AudioEngine::position`.

pub mod audio;
pub mod mix;
pub mod pcm;
pub mod video;

pub use audio::{AudioEngine, Engine, NullEngine};
pub use mix::{AudioSegment, Timeline};
pub use pcm::{Pcm, PcmCache};
pub use video::{FrameStream, LayerVideo, Picture, ScriptVideo, VideoFrame};
