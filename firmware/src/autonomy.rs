//! host に依存しない短い反応。単調時計で進め、描画やセンサの I/O を行わない。

use crate::settings::Settings;
use protocol::{Emote, Expression, EyeStyle, Gaze};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    Tap { x: u16, y: u16 },
    Stroke { right: bool },
    Nearby,
}

#[derive(Default)]
pub struct Autonomy {
    emote: Option<Emote>,
    until_ms: u64,
    speech: Option<&'static str>,
    speech_until_ms: u64,
    next_idle_ms: u64,
    next_reaction_ms: u64,
    random: u32,
    dark: bool,
    near: bool,
}

impl Autonomy {
    pub fn emote(&self) -> Option<&Emote> {
        self.emote.as_ref()
    }
    pub fn speech(&self) -> Option<&'static str> {
        self.speech
    }

    fn clear(&mut self) -> bool {
        let changed = self.emote.is_some() || self.speech.is_some();
        self.emote = None;
        self.speech = None;
        changed
    }

    pub fn tick(&mut self, now_ms: u64, settings: Settings) -> bool {
        if !settings.enabled {
            self.next_idle_ms = now_ms.saturating_add(settings.pace.interval_ms());
            return self.clear();
        }
        let mut changed = false;
        if self.emote.is_some() && now_ms >= self.until_ms {
            self.emote = None;
            changed = true;
        }
        if self.speech.is_some() && (now_ms >= self.speech_until_ms || !settings.speech) {
            self.speech = None;
            changed = true;
        }
        if settings.sensors && settings.dark_quiet && self.dark {
            // 暗所では反復動作を止める。照度値から実時間や就寝を推測しない。
            self.next_idle_ms = now_ms.saturating_add(settings.pace.interval_ms());
            return changed;
        }
        if self.next_idle_ms == 0 {
            self.next_idle_ms = now_ms.saturating_add(3_000);
        } else if now_ms >= self.next_idle_ms && self.emote.is_none() {
            // 演出の選択だけに使う。入力の時刻も混ぜ、再起動後の単純な反復を避ける。
            self.random = self
                .random
                .wrapping_add(now_ms as u32)
                .wrapping_add(0x9e3779b9);
            self.random ^= self.random << 13;
            self.random ^= self.random >> 17;
            self.random ^= self.random << 5;
            let (expression, gaze, text) = match self.random % 6 {
                0 => (Expression::Curious, Gaze::Left, "Hmm..."),
                1 => (Expression::Curious, Gaze::Right, "What's over there?"),
                2 => (Expression::Happy, Gaze::Center, "Hello!"),
                3 => (Expression::Wink, Gaze::Center, "Taking a break?"),
                4 => (Expression::Calm, Gaze::Up, "A quiet moment."),
                _ => (Expression::Sleepy, Gaze::Down, "Zzz..."),
            };
            self.show(expression, gaze, text, now_ms, settings);
            changed = true;
        }
        changed
    }

    fn show(
        &mut self,
        expression: Expression,
        gaze: Gaze,
        text: &'static str,
        now_ms: u64,
        settings: Settings,
    ) {
        self.emote = Some(Emote {
            expression,
            gaze,
            eyes: EyeStyle::Auto,
            intensity: 30,
            duration_ms: 2_000,
        });
        self.until_ms = now_ms.saturating_add(2_000);
        self.speech = settings.speech.then_some(text);
        self.speech_until_ms = now_ms.saturating_add(3_500);
        self.next_idle_ms = now_ms.saturating_add(settings.pace.interval_ms());
    }

    pub fn react(&mut self, trigger: Trigger, now_ms: u64, settings: Settings) -> bool {
        if !settings.enabled || now_ms < self.next_reaction_ms {
            return false;
        }
        let (expression, gaze, text) = match trigger {
            Trigger::Tap { x, y } if settings.touch => (
                Expression::Surprised,
                Gaze::Point {
                    x: ((i32::from(x.min(319)) - 160) / 2).clamp(-70, 70) as i8,
                    y: ((i32::from(y.min(239)) - 120) / 2).clamp(-50, 50) as i8,
                },
                "Oh, hello!",
            ),
            Trigger::Stroke { right } if settings.touch => (
                Expression::Grin,
                if right { Gaze::Right } else { Gaze::Left },
                "That feels nice!",
            ),
            Trigger::Nearby if settings.sensors => (Expression::Happy, Gaze::Center, "Hi there!"),
            _ => return false,
        };
        self.show(expression, gaze, text, now_ms, settings);
        self.next_reaction_ms = now_ms.saturating_add(700);
        true
    }

    /// 近接の立上りにだけ反応する。手を置いたまま合図を繰り返さない。
    pub fn sensors(&mut self, near: bool, dark: bool, now_ms: u64, settings: Settings) -> bool {
        let entered = near && !self.near;
        self.near = near;
        self.dark = dark;
        entered && self.react(Trigger::Nearby, now_ms, settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_works_without_host_and_is_bounded() {
        let mut model = Autonomy::default();
        for now in (0..120_000).step_by(100) {
            model.tick(now, Settings::default());
            if let Some(emote) = model.emote() {
                assert!(emote.validate().is_ok());
                assert!(emote.intensity <= 30);
            }
        }
        assert!(model.next_idle_ms > 100_000);
    }

    #[test]
    fn disabled_output_is_removed_and_reactions_do_not_bypass_settings() {
        let mut model = Autonomy::default();
        let mut settings = Settings::default();
        assert!(model.react(Trigger::Stroke { right: true }, 100, settings));
        settings.enabled = false;
        assert!(model.tick(200, settings));
        assert_eq!(model.emote(), None);
        assert_eq!(model.speech(), None);
        assert!(!model.react(Trigger::Nearby, 2_000, settings));
        settings.enabled = true;
        settings.touch = false;
        assert!(!model.react(Trigger::Tap { x: 0, y: 0 }, 3_000, settings));
    }

    #[test]
    fn proximity_requires_an_edge_and_darkness_suppresses_idle() {
        let mut model = Autonomy::default();
        let settings = Settings::default();
        assert!(model.sensors(true, false, 0, settings));
        assert!(!model.sensors(true, false, 1_000, settings));
        model.sensors(false, true, 2_000, settings);
        for now in (2_000..60_000).step_by(100) {
            model.tick(now, settings);
            assert_eq!(model.emote(), None);
        }
    }

    #[test]
    fn reaction_expires_and_speech_can_be_disabled_immediately() {
        let mut model = Autonomy::default();
        let settings = Settings::default();
        model.react(Trigger::Tap { x: 160, y: 120 }, 100, settings);
        assert!(model.speech().is_some());
        model.tick(
            200,
            Settings {
                speech: false,
                ..settings
            },
        );
        assert_eq!(model.speech(), None);
        assert!(model.emote().is_some());
        model.tick(2_100, settings);
        assert_eq!(model.emote(), None);
    }
}
