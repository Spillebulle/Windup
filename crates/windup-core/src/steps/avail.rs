//! Whether Steps can work here, and if not, why (style guide §7.6).

use crate::capture::backend::{StepRoute, WlPointer};

pub const NO_STEPS_SOURCE: &str = "This desktop does not let applications read the screen.";
pub const NO_POINTER_ON_SWAY: &str = "Sway does not tell applications where the pointer is, so steps show the pointer itself and no marker.";
pub const NO_POINTER_HERE: &str =
    "This desktop does not tell applications where the pointer is, so steps have no marker.";

pub fn steps_available(order: &[StepRoute]) -> Result<(), &'static str> {
    if order.iter().all(|r| *r == StepRoute::None) {
        Err(NO_STEPS_SOURCE)
    } else {
        Ok(())
    }
}

/// Whether click markers can be placed on the pictures this route takes.
pub fn markers_available(route: StepRoute) -> Result<(), &'static str> {
    match route {
        StepRoute::Screencopy {
            pointer: WlPointer::SwayFocus,
            ..
        } => Err(NO_POINTER_ON_SWAY),
        StepRoute::Screencopy {
            pointer: WlPointer::Unknown,
            ..
        } => Err(NO_POINTER_HERE),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_route_says_so() {
        assert_eq!(steps_available(&[StepRoute::None]), Err(NO_STEPS_SOURCE));
        assert_eq!(steps_available(&[]), Err(NO_STEPS_SOURCE));
        assert_eq!(steps_available(&[StepRoute::Gdi]), Ok(()));
        assert_eq!(
            steps_available(&[StepRoute::KWin, StepRoute::ScreenCast]),
            Ok(())
        );
    }

    #[test]
    fn markers_need_a_pointer() {
        let s = |pointer| StepRoute::Screencopy {
            ext: false,
            pointer,
        };
        assert_eq!(
            markers_available(s(WlPointer::SwayFocus)),
            Err(NO_POINTER_ON_SWAY)
        );
        assert_eq!(
            markers_available(s(WlPointer::Unknown)),
            Err(NO_POINTER_HERE)
        );
        assert_eq!(markers_available(s(WlPointer::Hyprland)), Ok(()));
        assert_eq!(markers_available(s(WlPointer::CursorSession)), Ok(()));
        assert_eq!(markers_available(StepRoute::Gdi), Ok(()));
    }
}
