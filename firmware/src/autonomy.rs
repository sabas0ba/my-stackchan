//! host に依存しない短い反応。単調時計で進め、描画やセンサの I/O を行わない。

use crate::settings::{Pace, Settings};
use protocol::{Emote, Expression, EyeStyle, Gaze};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    Tap { x: u16, y: u16 },
    Stroke { right: bool },
    Nearby,
}

const TAP: &[&str] = &[
    "こんにちは！",
    "呼んだ？",
    "なあに？",
    "ここにいるよ。",
    "会えてうれしいな。",
];
const TAP_AGAIN: &[&str] = &[
    "聞いてるよ。",
    "くすぐったいよ。",
    "はい、こちらです！",
    "どうしたの？",
];
const STROKE: &[&str] = &[
    "気持ちいいな。",
    "なでてくれて、ありがとう。",
    "ふふ、うれしい。",
    "なんだか安心するね。",
    "のんびりしよう。",
];
const STROKE_AGAIN: &[&str] = &[
    "もう少し、このまま。",
    "すっかりごきげん。",
    "やさしい手だね。",
    "ありがとう、うれしいな。",
];
const NEARBY: &[&str] = &[
    "あ、こんにちは！",
    "そばにいるよ。",
    "何かお話しする？",
    "見つけた！",
];
const IDLE: &[&str] = &[
    "ちょっと、ひと休み。",
    "今日はどんな日？",
    "何か楽しいこと、あるかな。",
    "ゆっくりいこう。",
    "あちらが気になるな。",
    "ふわあ、のんびり。",
    "ここで待ってるね。",
    "うん、いい感じ。",
];

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
    started_ms: u64,
    stage: u8,
    settle: Option<Emote>,
    last_speech_ms: Option<u64>,
    last_text: Option<&'static str>,
    last_touch: Option<(u8, u64)>,
    last_idle: Option<usize>,
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
        self.settle = None;
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
            self.settle = None;
            changed = true;
        } else if let Some(mut settle) = self.settle {
            let elapsed = now_ms.saturating_sub(self.started_ms);
            let stage = if elapsed >= 1_100 {
                2
            } else if elapsed >= 400 {
                1
            } else {
                0
            };
            if stage > self.stage {
                if stage == 2 {
                    settle.gaze = Gaze::Center;
                    settle.eyes = EyeStyle::Auto;
                }
                self.emote = Some(settle);
                self.stage = stage;
                changed = true;
            }
        }
        if let Some(emote) = &mut self.emote {
            let intensity = Self::intensity(settings);
            if emote.intensity != intensity {
                emote.intensity = intensity;
                changed = true;
            }
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
            let mut choice = self.random(now_ms) as usize % 8;
            if self.last_idle == Some(choice) {
                choice = (choice + 1) % 8;
            }
            self.last_idle = Some(choice);
            let (expression, gaze) = match choice {
                0 => (Expression::Curious, Gaze::Left),
                1 => (Expression::Curious, Gaze::Right),
                2 => (Expression::Happy, Gaze::Center),
                3 => (Expression::Wink, Gaze::Center),
                4 => (Expression::Calm, Gaze::Up),
                5 => (Expression::Sleepy, Gaze::Down),
                6 => (Expression::Playful, Gaze::Left),
                _ => (Expression::Grin, Gaze::Right),
            };
            let text = self.choose(IDLE, now_ms);
            let previous_speech = (self.last_speech_ms, self.last_text);
            self.show(expression, gaze, text, now_ms, settings);
            // 待機中は半分を無言の仕草とし、毎回吹き出しで画面を占めない。
            if choice.is_multiple_of(2) {
                self.speech = None;
                (self.last_speech_ms, self.last_text) = previous_speech;
            }
            changed = true;
        }
        changed
    }

    fn intensity(settings: Settings) -> u8 {
        match settings.expression {
            Pace::Quiet => 10,
            Pace::Normal => 20,
            Pace::Lively => 30,
        }
    }

    fn random(&mut self, now_ms: u64) -> u32 {
        self.random = self
            .random
            .wrapping_add(now_ms as u32)
            .wrapping_add(0x9e3779b9);
        self.random ^= self.random << 13;
        self.random ^= self.random >> 17;
        self.random ^= self.random << 5;
        self.random
    }

    fn choose(&mut self, choices: &'static [&'static str], now_ms: u64) -> &'static str {
        let mut index = self.random(now_ms) as usize % choices.len();
        if self.last_text == Some(choices[index]) {
            index = (index + 1) % choices.len();
        }
        choices[index]
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
            intensity: Self::intensity(settings),
            duration_ms: 2_000,
        });
        self.until_ms = now_ms.saturating_add(2_000);
        self.started_ms = now_ms;
        self.stage = 0;
        self.settle = self.emote.map(|mut emote| {
            emote.expression = match expression {
                Expression::Surprised | Expression::Curious => Expression::Happy,
                Expression::Grin | Expression::Sleepy => Expression::Calm,
                _ => expression,
            };
            emote.eyes = if expression == Expression::Grin {
                EyeStyle::Closed
            } else {
                EyeStyle::Auto
            };
            emote
        });
        let interval = match settings.speech_pace {
            Pace::Quiet => 30_000,
            Pace::Normal => 8_000,
            Pace::Lively => 3_500,
        };
        // 設定変更直後も現在の頻度を使う。無言の動作は発話時刻を更新しない。
        if settings.speech
            && self
                .last_speech_ms
                .is_none_or(|last| now_ms.saturating_sub(last) >= interval)
        {
            self.speech = Some(text);
            self.last_text = Some(text);
            self.speech_until_ms = now_ms.saturating_add(3_500);
            self.last_speech_ms = Some(now_ms);
        } else if !settings.speech {
            self.speech = None;
        }
        self.next_idle_ms = now_ms.saturating_add(settings.pace.interval_ms());
    }

    pub fn react(&mut self, trigger: Trigger, now_ms: u64, settings: Settings) -> bool {
        if !settings.enabled || now_ms < self.next_reaction_ms {
            return false;
        }
        let repeated = |kind| {
            self.last_touch
                .is_some_and(|(last, time)| last == kind && now_ms.saturating_sub(time) < 5_000)
        };
        let (expression, gaze, choices) = match trigger {
            Trigger::Tap { x, y } if settings.touch => (
                Expression::Surprised,
                Gaze::Point {
                    x: ((i32::from(x.min(319)) - 160) / 2).clamp(-70, 70) as i8,
                    y: ((i32::from(y.min(239)) - 120) / 2).clamp(-50, 50) as i8,
                },
                if repeated(0) { TAP_AGAIN } else { TAP },
            ),
            Trigger::Stroke { right } if settings.touch => (
                Expression::Grin,
                if right { Gaze::Right } else { Gaze::Left },
                if repeated(1) { STROKE_AGAIN } else { STROKE },
            ),
            Trigger::Nearby if settings.sensors => (Expression::Happy, Gaze::Center, NEARBY),
            _ => return false,
        };
        match trigger {
            Trigger::Tap { .. } => self.last_touch = Some((0, now_ms)),
            Trigger::Stroke { .. } => self.last_touch = Some((1, now_ms)),
            Trigger::Nearby => {}
        }
        let text = self.choose(choices, now_ms);
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
    fn review_speech_pace_changes_apply_to_the_current_wait() {
        for (before, after, deadline) in [
            (Pace::Lively, Pace::Quiet, 30_000),
            (Pace::Quiet, Pace::Lively, 3_500),
        ] {
            let mut model = Autonomy::default();
            let mut settings = Settings {
                speech_pace: before,
                ..Settings::default()
            };
            model.show(Expression::Happy, Gaze::Center, "最初", 0, settings);
            settings.speech_pace = after;
            model.show(
                Expression::Happy,
                Gaze::Center,
                "早すぎる",
                deadline - 1,
                settings,
            );
            assert_eq!(model.speech(), Some("最初"));
            model.show(Expression::Happy, Gaze::Center, "次", deadline, settings);
            assert_eq!(model.speech(), Some("次"));
        }
    }

    #[test]
    fn expressions_transition_and_cancel_without_late_reappearance() {
        let mut model = Autonomy::default();
        let settings = Settings::default();
        model.react(Trigger::Tap { x: 0, y: 120 }, 100, settings);
        assert_eq!(model.emote().unwrap().expression, Expression::Surprised);
        model.tick(500, settings);
        assert_eq!(model.emote().unwrap().expression, Expression::Happy);
        assert_ne!(model.emote().unwrap().gaze, Gaze::Center);
        model.tick(1200, settings);
        assert_eq!(model.emote().unwrap().gaze, Gaze::Center);
        model.tick(
            1300,
            Settings {
                enabled: false,
                ..settings
            },
        );
        model.tick(1400, settings);
        assert!(model.emote().is_none());
    }

    #[test]
    fn speech_varies_without_repeating_or_flooding_and_respects_intensity() {
        let mut model = Autonomy::default();
        let settings = Settings {
            expression: Pace::Quiet,
            ..Settings::default()
        };
        let mut last = None;
        for now in (0..100_000).step_by(8_000) {
            assert!(model.react(Trigger::Tap { x: 160, y: 120 }, now, settings));
            let text = model.speech();
            assert_ne!(text, last);
            assert_eq!(model.emote().unwrap().intensity, 10);
            assert!(model.react(Trigger::Tap { x: 160, y: 120 }, now + 800, settings));
            assert_eq!(model.speech(), text);
            last = text;
        }
        for pool in [TAP, TAP_AGAIN, STROKE, STROKE_AGAIN, NEARBY, IDLE] {
            for text in pool {
                assert!(crate::font::text_width(text) <= 256, "{text}");
            }
        }
    }

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
