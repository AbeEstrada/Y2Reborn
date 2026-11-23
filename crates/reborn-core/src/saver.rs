//! Battery Saver (Y2Linux power plan 2026-11, phase 5; Android's Battery Saver): a dimmer screen
//! that turns off sooner and no animation, on request or automatically on a low battery. The music
//! itself never changes. Settings → Display → Battery Saver.
use crate::{platform::ChargingState, AppModel, BRIGHTNESS_MIN};
use serde::{Deserialize, Serialize};

/// At or below this percentage on the battery, Automatic turns the saver on.
pub const SAVER_PERCENT: u8 = 15;
/// The brightest the screen gets while the saver is on (percent of the panel's maximum).
pub const SAVER_BRIGHTNESS: u8 = 30;
/// The longest the screen stays on without a button press while the saver is on.
pub const SAVER_TIMEOUT_SECONDS: u32 = 15;

/// When the saver is on. Automatic by default: it changes nothing until the battery is low.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BatterySaver {
    Off,
    #[default]
    Automatic,
    On,
}

impl BatterySaver {
    pub const ALL: [Self; 3] = [Self::Off, Self::Automatic, Self::On];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Automatic => "At 15 %",
            Self::On => "On",
        }
    }

    /// The picker row's key part (`saver_set:<key>`).
    pub fn key(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Automatic => "automatic",
            Self::On => "on",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.key() == key)
    }
}

impl AppModel {
    /// Whether the saver is on now: chosen On, or Automatic with the battery at or below
    /// `SAVER_PERCENT` and nothing charging it (a cable in turns Automatic off again).
    pub fn battery_saver_active(&self) -> bool {
        match self.settings.battery_saver {
            BatterySaver::Off => false,
            BatterySaver::On => true,
            BatterySaver::Automatic => {
                let battery = &self.platform.battery;
                !battery.cable
                    && battery.charging == ChargingState::OnBattery
                    && battery.percent.is_some_and(|p| p <= SAVER_PERCENT)
            }
        }
    }

    /// The brightness to set on the panel: the listener's, at most `SAVER_BRIGHTNESS` while the
    /// saver is on (the setting itself is kept).
    pub fn effective_brightness(&self) -> u8 {
        let chosen = self.settings.brightness;
        if self.battery_saver_active() {
            chosen.clamp(BRIGHTNESS_MIN, SAVER_BRIGHTNESS)
        } else {
            chosen
        }
    }

    /// The screen's timeout in seconds (0 = never): at most `SAVER_TIMEOUT_SECONDS` while the saver
    /// is on, Never included.
    pub fn effective_screen_timeout(&self) -> u32 {
        let chosen = self.settings.screen_timeout_seconds;
        if !self.battery_saver_active() {
            chosen
        } else if chosen == 0 {
            SAVER_TIMEOUT_SECONDS
        } else {
            chosen.min(SAVER_TIMEOUT_SECONDS)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(saver: BatterySaver, percent: Option<u8>, charging: ChargingState) -> AppModel {
        let mut m = AppModel::default();
        m.settings.battery_saver = saver;
        m.settings.brightness = 80;
        m.settings.screen_timeout_seconds = 60;
        m.platform.battery.percent = percent;
        m.platform.battery.charging = charging;
        m.platform.battery.cable = charging != ChargingState::OnBattery;
        m
    }

    #[test]
    fn automatic_turns_on_at_15_percent_on_the_battery_only() {
        use ChargingState::*;
        let on = |p, c| model(BatterySaver::Automatic, p, c).battery_saver_active();
        assert!(!on(Some(16), OnBattery));
        assert!(on(Some(15), OnBattery));
        assert!(!on(Some(10), Charging));
        assert!(!on(None, OnBattery));
        assert!(!on(Some(10), Unknown));
        assert!(model(BatterySaver::On, Some(90), Charging).battery_saver_active());
        assert!(!model(BatterySaver::Off, Some(5), OnBattery).battery_saver_active());
    }

    #[test]
    fn the_saver_dims_and_shortens_but_keeps_the_settings() {
        let mut m = model(BatterySaver::On, Some(50), ChargingState::OnBattery);
        assert_eq!(m.effective_brightness(), SAVER_BRIGHTNESS);
        assert_eq!(m.effective_screen_timeout(), SAVER_TIMEOUT_SECONDS);
        m.settings.brightness = 10;
        m.settings.screen_timeout_seconds = 0;
        assert_eq!(m.effective_brightness(), 10);
        assert_eq!(m.effective_screen_timeout(), SAVER_TIMEOUT_SECONDS);
        assert_eq!(m.settings.screen_timeout_seconds, 0);
        m.settings.battery_saver = BatterySaver::Off;
        assert_eq!(m.effective_brightness(), 10);
        assert_eq!(m.effective_screen_timeout(), 0);
    }

    #[test]
    fn older_settings_read_as_automatic_and_keys_round_trip() {
        let old: crate::Settings = serde_json::from_value(serde_json::json!({
            "volume": 35, "screen_timeout_seconds": 60, "music_directory": "/data/music"
        }))
        .unwrap();
        assert_eq!(old.battery_saver, BatterySaver::Automatic);
        for s in BatterySaver::ALL {
            assert_eq!(BatterySaver::from_key(s.key()), Some(s));
        }
    }
}
