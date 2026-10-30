//! The camera on GNOME and the KDE AppImage: the ScreenCast portal's
//! streams, held open for the whole Steps session (one dialog, or none with
//! video's restore token), each keeping a recent frame. The pointer is the
//! streams' cursor metadata where the portal offers it.

use std::time::{Duration, Instant};

use windup_core::capture::geometry::{DRect, MonitorShot};

use crate::capture::CaptureError;
use crate::linux::video::{ScreenCastStills, open_screencast_stills};
use crate::steps::camera::{Camera, Shot};
use crate::video::live::TokenSlot;

/// How long a shot waits for a frame newer than its press. A screen that
/// has not changed sends none, and then the newest copy (at most
/// `STILL_EVERY` old) is what it shows.
const FRESH_WAIT: Duration = Duration::from_millis(300);
/// How long a shot waits for a stream's first frame at all.
const FIRST_WAIT: Duration = Duration::from_secs(3);
/// How often a waiting shot looks.
const LOOK_EVERY: Duration = Duration::from_millis(10);

/// The stream a step pictures, from each stream's pointer and when it last
/// changed: the one whose cursor metadata has the pointer (the latest to
/// change, should two still hold one), else the first, without a pointer.
fn pick_stream(cursors: &[(Option<[f32; 2]>, u64)]) -> (usize, Option<[f32; 2]>) {
    cursors
        .iter()
        .enumerate()
        .filter_map(|(i, (p, stamp))| Some((*stamp, i, (*p)?)))
        .max_by_key(|(stamp, _, _)| *stamp)
        .map_or((0, None), |(_, i, p)| (i, Some(p)))
}

/// The portal camera.
pub struct PortalCamera {
    stills: ScreenCastStills,
}

impl PortalCamera {
    /// Starts the portal session with the restore token in `token`, which
    /// holds the next one once the portal has answered (the caller saves
    /// it, as video does). Blocks while the dialog is up.
    pub fn open(token: &TokenSlot) -> Result<PortalCamera, String> {
        open_screencast_stills(token)
            .map(|stills| PortalCamera { stills })
            .map_err(|e| crate::steps::camera::describe(&e))
    }

    /// Stream `i`'s frame from no earlier than `asked`, or its newest once
    /// `FRESH_WAIT` has passed; an error once the streams have ended or no
    /// frame came within `FIRST_WAIT`. Never waits longer.
    fn frame(&self, i: usize, asked: Instant) -> Result<image::RgbaImage, CaptureError> {
        loop {
            if let Some(why) = self.stills.ended() {
                return Err(CaptureError::Failed(why));
            }
            let waited = asked.elapsed();
            match self.stills.frame(i) {
                Some((image, at)) if at >= asked || waited >= FRESH_WAIT => return Ok(image),
                _ if waited >= FIRST_WAIT => {
                    return Err(CaptureError::Failed(
                        "the shared screen sent no picture".into(),
                    ));
                }
                _ => std::thread::sleep(LOOK_EVERY),
            }
        }
    }
}

impl Camera for PortalCamera {
    fn monitor(&mut self, _at: Option<[f32; 2]>) -> Option<DRect> {
        let (i, _) = pick_stream(&self.stills.cursors());
        self.stills.layout(i)
    }

    fn shoot(&mut self, _at: Option<[f32; 2]>) -> Result<Shot, CaptureError> {
        let (i, pointer) = pick_stream(&self.stills.cursors());
        let asked = Instant::now();
        self.stills.ask(i);
        let image = self.frame(i, asked)?;
        let layout = self
            .stills
            .layout(i)
            .ok_or_else(|| CaptureError::Failed("the portal shared no screen".into()))?;
        let (w, h) = (image.width() as f32, image.height() as f32);
        let m = MonitorShot {
            layout,
            image,
            name: format!("Screen {}", i + 1),
            scale: 1.0,
            refresh_hz: None,
        };
        let mut shot = Shot::of(m, None);
        // Already in the frame's pixels; a frame of another size since (the
        // screen's resolution changed) may no longer hold it.
        shot.pointer = pointer.filter(|p| p[0] < w && p[1] < h);
        Ok(shot)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stream_with_the_pointer_is_pictured() {
        let cursors = [(None, 0), (Some([10.0, 20.0]), 3)];
        assert_eq!(pick_stream(&cursors), (1, Some([10.0, 20.0])));
    }

    #[test]
    fn of_two_streams_holding_a_pointer_the_latest_to_change_wins() {
        let cursors = [(Some([5.0, 5.0]), 7), (Some([10.0, 20.0]), 3)];
        assert_eq!(pick_stream(&cursors), (0, Some([5.0, 5.0])));
    }

    #[test]
    fn without_a_pointer_the_first_stream_is_pictured() {
        assert_eq!(pick_stream(&[(None, 2), (None, 9)]), (0, None));
        assert_eq!(pick_stream(&[]), (0, None));
    }
}
