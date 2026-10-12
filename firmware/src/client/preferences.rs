//! 自律動作の設定画面。変更は即時反映し、保存は利用者の明示操作で要求する。

use super::{Panel, Plugin};
use crate::settings::Settings;
use protocol::{Card, Emote};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StorageStatus {
    #[default]
    Unavailable,
    Ready,
    Loaded,
    Recovered,
    Unsaved,
    Saving,
    Saved,
    Failed,
}

impl StorageStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Unavailable => "SD unavailable / RAM only",
            Self::Ready => "SD ready / no settings file",
            Self::Loaded => "Settings loaded from SD",
            Self::Recovered => "Loaded SD backup / SAVE",
            Self::Unsaved => "Changed / SAVE on page 2",
            Self::Saving => "Saving to SD...",
            Self::Saved => "Saved to SD",
            Self::Failed => "SD error / RAM unchanged",
        }
    }
}

#[derive(Default)]
pub struct Preferences {
    pub settings: Settings,
    pub status: StorageStatus,
    pub save_requested: bool,
    page: bool,
    open_until_ms: Option<u64>,
}

impl Plugin for Preferences {
    fn name(&self) -> &'static str {
        "SETTINGS"
    }
    fn tick(&mut self, now_ms: u64) {
        if self.open_until_ms.is_some_and(|until| now_ms >= until) {
            self.open_until_ms = None;
        }
    }
    fn open(&mut self, now_ms: u64) {
        self.page = false;
        self.open_until_ms = Some(now_ms.saturating_add(60_000));
    }
    fn card_tap(&mut self, _now_ms: u64) {}
    fn button(&mut self, index: usize, now_ms: u64) {
        if self.open_until_ms.is_none() {
            return;
        }
        self.open_until_ms = Some(now_ms.saturating_add(60_000));
        let before = self.settings;
        match (self.page, index) {
            (false, 0) => self.settings.enabled = !self.settings.enabled,
            (false, 1) => self.settings.speech = !self.settings.speech,
            (false, 2) => self.settings.motion = !self.settings.motion,
            (false, 3) => self.settings.led = !self.settings.led,
            (false, 4) => self.page = true,
            (false, 5) => self.open_until_ms = None,
            (true, 0) => self.settings.touch = !self.settings.touch,
            (true, 1) => self.settings.sensors = !self.settings.sensors,
            (true, 2) => self.settings.dark_quiet = !self.settings.dark_quiet,
            (true, 3) => self.settings.pace = self.settings.pace.next(),
            (true, 4) => {
                self.save_requested = true;
                self.status = StorageStatus::Saving;
            }
            (true, 5) => self.page = false,
            _ => {}
        }
        if self.settings != before {
            self.status = StorageStatus::Unsaved;
        }
    }
    fn card(&self, _now_ms: u64) -> Option<Card> {
        None
    }
    fn panel(&self, _now_ms: u64) -> Option<Panel> {
        self.open_until_ms?;
        let s = self.settings;
        let buttons = if self.page {
            [
                if s.touch { "TOUCH ON" } else { "TOUCH OFF" },
                if s.sensors { "SENSE ON" } else { "SENSE OFF" },
                if s.dark_quiet { "DARK ON" } else { "DARK OFF" },
                s.pace.label(),
                "SAVE",
                "BACK",
            ]
        } else {
            [
                if s.enabled { "AUTO ON" } else { "AUTO OFF" },
                if s.speech { "TEXT ON" } else { "TEXT OFF" },
                if s.motion { "MOVE ON" } else { "MOVE OFF" },
                if s.led { "LED ON" } else { "LED OFF" },
                "NEXT",
                "CLOSE",
            ]
        };
        Some(Panel::new(
            [
                if self.page {
                    "SETTINGS 2/2"
                } else {
                    "SETTINGS 1/2"
                },
                self.status.label(),
            ],
            &buttons,
        ))
    }
    fn take_emote(&mut self) -> Option<Emote> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disabling_reactions_does_not_disable_settings_or_implicitly_save() {
        let mut preferences = Preferences::default();
        preferences.open(0);
        preferences.button(0, 1);
        assert!(!preferences.settings.enabled);
        assert!(!preferences.save_requested);
        assert!(preferences.panel(1).is_some());
        preferences.button(4, 2);
        preferences.button(4, 3);
        assert!(preferences.save_requested);
        assert_eq!(preferences.status, StorageStatus::Saving);
        preferences.tick(60_003);
        assert!(preferences.panel(60_003).is_none());
        assert!(!preferences.settings.enabled);
    }
}
