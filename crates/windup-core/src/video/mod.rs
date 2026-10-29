//! Screen recording that needs no platform: which formats go together and
//! how fast the encoder should run.

pub mod avail;
pub mod command;
pub mod compose;
pub mod format;
pub mod pacing;
pub mod pcm;
pub mod place;
pub mod probe;
pub mod recorder;
pub mod screencast;

pub use command::{AudioIn, SegmentJob, Source, join_args, segment_args, segment_ext};
pub use compose::Composer;
pub use pacing::Pacer;
pub use place::place_bar;
pub use recorder::{Finish, RecState, Recorder};

pub use format::{
    AudioCodec, Bitrate, Codec, Container, FIXED_BITRATES, GIF_COLOURS, audio_for, auto_bitrate,
    codecs_for, even_size, frame_rates,
};
pub use probe::{
    Encoder, Probe, audio_encoder, audio_probe_args, cache_key, candidates, pick_encoder,
    probe_args,
};
