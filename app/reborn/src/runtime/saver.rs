//! Battery Saver on the player (power plan 2026-11, phase 5): the panel's brightness and Reborn's
//! motion follow `AppModel::battery_saver_active` whenever the battery or the choice changes.
use crate::*;

impl Runtime {
    /// The saver was `was` before the battery reading or the choice changed: if it turned on or
    /// off, set the panel and the motion to match (the listener's settings stay as they are).
    pub(crate) fn follow_battery_saver(&mut self, was: bool) {
        let now = self.model.battery_saver_active();
        if now == was {
            return;
        }
        if !self.headless {
            self.ui.ui_motion = !now;
            if self.model.platform.brightness_available {
                let _ = power::set_brightness(self.model.effective_brightness());
            }
        }
        if now && self.model.settings.battery_saver == reborn_core::BatterySaver::Automatic {
            self.ui
                .flash("Battery Saver is on: a dimmer screen and no animation.");
        }
        self.dirty.mark_render();
    }
}
