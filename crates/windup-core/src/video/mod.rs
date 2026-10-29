//! Screen recording that needs no platform: which formats go together and
//! how fast the encoder should run.

pub mod format;

pub use format::{
    AudioCodec, Bitrate, Codec, Container, FIXED_BITRATES, GIF_COLOURS, audio_for, auto_bitrate,
    codecs_for, even_size, frame_rates,
};
