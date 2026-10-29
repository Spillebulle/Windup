//! Screen recording that needs no platform: which formats go together and
//! how fast the encoder should run.

pub mod format;
pub mod probe;

pub use format::{
    AudioCodec, Bitrate, Codec, Container, FIXED_BITRATES, GIF_COLOURS, audio_for, auto_bitrate,
    codecs_for, even_size, frame_rates,
};
pub use probe::{
    Encoder, Probe, audio_encoder, audio_probe_args, cache_key, candidates, pick_encoder,
    probe_args,
};
