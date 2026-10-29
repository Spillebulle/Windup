//! Screen recording: the bundled ffmpeg and what it can encode.

pub(crate) mod ffmpeg;
pub(crate) mod pipe;
pub(crate) mod run;
pub(crate) mod test_source;
#[cfg(target_os = "windows")]
pub(crate) mod windows;
#[cfg(target_os = "linux")]
pub(crate) mod x11;
