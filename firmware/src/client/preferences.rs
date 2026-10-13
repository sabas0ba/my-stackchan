//! 自律動作の設定画面。変更は即時反映し、保存は利用者の明示操作で要求する。

use super::{Icon, Panel, Plugin, icon_button, labeled_button};
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
    page: u8,
    open_until_ms: Option<u64>,
}

impl Plugin for Preferences {
    fn name(&self) -> &'static str {
        "SETTINGS"
    }
    fn icon(&self) -> Icon {
        Icon::Gear
    }
    fn tick(&mut self, now_ms: u64) {
        if self.open_until_ms.is_some_and(|until| now_ms >= until) {
            self.open_until_ms = None;
        }
    }
    fn open(&mut self, now_ms: u64) {
        self.page = 0;
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
            (0, 0) => self.settings.enabled = !self.settings.enabled,
            (0, 1) => self.settings.speech = !self.settings.speech,
            (0, 2) => self.settings.motion = !self.settings.motion,
            (0, 3) => self.settings.led = !self.settings.led,
            (0, 4) => self.page = 1,
            (0, 5) => self.open_until_ms = None,
            (1, 0) => self.settings.touch = !self.settings.touch,
            (1, 1) => self.settings.sensors = !self.settings.sensors,
            (1, 2) => self.settings.dark_quiet = !self.settings.dark_quiet,
            (1, 3) => self.settings.pace = self.settings.pace.next(),
            (1 | 2, 4) => {
                self.save_requested = true;
                self.status = StorageStatus::Saving;
            }
            (1, 5) => self.page = 2,
            (2, 0) => self.settings.expression = self.settings.expression.next(),
            (2, 1) => self.settings.speech_pace = self.settings.speech_pace.next(),
            (2, 2) => self.page = 0,
            (2, 3) => self.page = 1,
            (2, 5) => self.open_until_ms = None,
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
        // 切替のボタンはピクトグラムと項目名を出し、ON を枠の塗りで示す。
        let toggle = |icon, label, on| labeled_button(icon, label, on);
        let buttons = if self.page == 1 {
            [
                toggle(Icon::Touch, "TOUCH", s.touch),
                toggle(Icon::Sense, "SENSE", s.sensors),
                toggle(Icon::Dark, "DARK", s.dark_quiet),
                labeled_button(Icon::Pace, s.pace.label(), false),
                icon_button(Icon::Save),
                icon_button(Icon::Forward),
            ]
        } else if self.page == 2 {
            use crate::settings::Pace;
            [
                labeled_button(
                    Icon::Face,
                    match s.expression {
                        Pace::Quiet => "表現：弱",
                        Pace::Normal => "表現：中",
                        Pace::Lively => "表現：強",
                    },
                    false,
                ),
                labeled_button(
                    Icon::Speech,
                    match s.speech_pace {
                        Pace::Quiet => "発話：少",
                        Pace::Normal => "発話：中",
                        Pace::Lively => "発話：多",
                    },
                    false,
                ),
                icon_button(Icon::First),
                icon_button(Icon::Back),
                icon_button(Icon::Save),
                icon_button(Icon::Close),
            ]
        } else {
            [
                toggle(Icon::Power, "AUTO", s.enabled),
                toggle(Icon::Speech, "TEXT", s.speech),
                toggle(Icon::Motion, "MOVE", s.motion),
                toggle(Icon::Light, "LED", s.led),
                icon_button(Icon::Forward),
                icon_button(Icon::Close),
            ]
        };
        Some(super::panel(
            [
                match self.page {
                    0 => "SETTINGS 1/3",
                    1 => "SETTINGS 2/3",
                    _ => "表現の設定 3/3",
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
