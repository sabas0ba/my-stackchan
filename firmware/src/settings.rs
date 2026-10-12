//! 自律動作の設定。保存先に依存させず、カード上の不正な値もここで拒否する。

use core::fmt::Write;

pub const MAX_CONFIG_BYTES: usize = 256;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pace {
    Quiet,
    #[default]
    Normal,
    Lively,
}

impl Pace {
    pub fn next(self) -> Self {
        match self {
            Self::Quiet => Self::Normal,
            Self::Normal => Self::Lively,
            Self::Lively => Self::Quiet,
        }
    }

    pub fn interval_ms(self) -> u64 {
        match self {
            Self::Quiet => 30_000,
            Self::Normal => 12_000,
            Self::Lively => 6_000,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Quiet => "quiet",
            Self::Normal => "normal",
            Self::Lively => "lively",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Settings {
    pub enabled: bool,
    pub speech: bool,
    pub motion: bool,
    pub led: bool,
    pub touch: bool,
    pub sensors: bool,
    pub dark_quiet: bool,
    pub pace: Pace,
    pub expression: Pace,
    pub speech_pace: Pace,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: true,
            speech: true,
            motion: true,
            led: true,
            touch: true,
            sensors: true,
            dark_quiet: true,
            pace: Pace::Normal,
            expression: Pace::Lively,
            speech_pace: Pace::Normal,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigError {
    TooLong,
    Invalid,
    Duplicate,
    Version,
}

impl Settings {
    /// ファイルの項目を一括で検証する。途中まで読めた設定を実機へ適用しない。
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        if text.len() > MAX_CONFIG_BYTES {
            return Err(ConfigError::TooLong);
        }
        let mut value = Self::default();
        let mut seen = 0u16;
        let mut version = "";
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, item) = line.split_once('=').ok_or(ConfigError::Invalid)?;
            let key = key.trim();
            let item = item.trim();
            let flag = || match item {
                "true" => Ok(true),
                "false" => Ok(false),
                _ => Err(ConfigError::Invalid),
            };
            let bit = match key {
                "version" => {
                    if !matches!(item, "1" | "2") {
                        return Err(ConfigError::Version);
                    }
                    version = item;
                    0
                }
                "enabled" => {
                    value.enabled = flag()?;
                    1
                }
                "speech" => {
                    value.speech = flag()?;
                    2
                }
                "motion" => {
                    value.motion = flag()?;
                    3
                }
                "led" => {
                    value.led = flag()?;
                    4
                }
                "touch" => {
                    value.touch = flag()?;
                    5
                }
                "sensors" => {
                    value.sensors = flag()?;
                    6
                }
                "dark_quiet" => {
                    value.dark_quiet = flag()?;
                    7
                }
                "pace" => {
                    value.pace = match item {
                        "quiet" => Pace::Quiet,
                        "normal" => Pace::Normal,
                        "lively" => Pace::Lively,
                        _ => return Err(ConfigError::Invalid),
                    };
                    8
                }
                "expression" | "speech_pace" => {
                    let pace = match item {
                        "quiet" => Pace::Quiet,
                        "normal" => Pace::Normal,
                        "lively" => Pace::Lively,
                        _ => return Err(ConfigError::Invalid),
                    };
                    if key == "expression" {
                        value.expression = pace;
                        9
                    } else {
                        value.speech_pace = pace;
                        10
                    }
                }
                _ => return Err(ConfigError::Invalid),
            };
            if seen & (1 << bit) != 0 {
                return Err(ConfigError::Duplicate);
            }
            seen |= 1 << bit;
        }
        if seen & 1 == 0 {
            return Err(ConfigError::Version);
        }
        if seen != if version == "1" { 0x1ff } else { 0x7ff } {
            return Err(ConfigError::Invalid);
        }
        Ok(value)
    }

    pub fn encode(self) -> heapless::String<MAX_CONFIG_BYTES> {
        let mut text = heapless::String::new();
        // 項目名と値の最大長は固定で、256 byte に収まる。
        write!(text,
            "version=2\nenabled={}\nspeech={}\nmotion={}\nled={}\ntouch={}\nsensors={}\ndark_quiet={}\npace={}\nexpression={}\nspeech_pace={}\n",
            self.enabled, self.speech, self.motion, self.led, self.touch,
            self.sensors, self.dark_quiet, self.pace.label(), self.expression.label(), self.speech_pace.label()
        ).expect("bounded settings encoding");
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_setting_round_trips() {
        for mask in 0..128u8 {
            for pace in [Pace::Quiet, Pace::Normal, Pace::Lively] {
                let value = Settings {
                    enabled: mask & 1 != 0,
                    speech: mask & 2 != 0,
                    motion: mask & 4 != 0,
                    led: mask & 8 != 0,
                    touch: mask & 16 != 0,
                    sensors: mask & 32 != 0,
                    dark_quiet: mask & 64 != 0,
                    pace,
                    ..Settings::default()
                };
                assert_eq!(Settings::parse(&value.encode()), Ok(value));
            }
        }
    }

    #[test]
    fn truncated_saved_settings_are_rejected() {
        let text = Settings::default().encode();
        for length in 0..text.len() - 1 {
            assert!(Settings::parse(&text[..length]).is_err(), "prefix {length}");
        }
    }

    #[test]
    fn version_one_migrates_without_losing_existing_preferences() {
        let legacy = "version=1\nenabled=true\nspeech=false\nmotion=false\nled=true\ntouch=true\nsensors=true\ndark_quiet=true\npace=quiet\n";
        let settings = Settings::parse(legacy).unwrap();
        assert!(!settings.speech && !settings.motion);
        assert_eq!(settings.pace, Pace::Quiet);
        assert_eq!(settings.expression, Pace::Lively);
        for expression in [Pace::Quiet, Pace::Normal, Pace::Lively] {
            for speech_pace in [Pace::Quiet, Pace::Normal, Pace::Lively] {
                let settings = Settings {
                    expression,
                    speech_pace,
                    ..settings
                };
                assert_eq!(Settings::parse(&settings.encode()), Ok(settings));
            }
        }
        assert!(Settings::parse(&legacy.replace("version=1", "version=2")).is_err());
    }

    #[test]
    fn invalid_files_never_partially_apply() {
        for text in [
            "",
            "enabled=false",
            "version=2",
            "version=1\nmotion=yes",
            "version=1\nenabled=false\nenabled=true",
            "version=1\npace=fast",
            "version=1\nunknown=true",
            "version=1\nversion=1",
        ] {
            assert!(Settings::parse(text).is_err(), "{text}");
        }
        assert_eq!(
            Settings::parse(&"x".repeat(MAX_CONFIG_BYTES + 1)),
            Err(ConfigError::TooLong)
        );
        assert_eq!(
            Settings::parse(&std::format!(
                "# settings\r\n{}",
                Settings::default()
                    .encode()
                    .replace("=", " = ")
                    .replace('\n', "\r\n")
            )),
            Ok(Settings::default())
        );
    }
}
