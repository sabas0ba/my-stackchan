//! 表示状態から機構部の安全な目標値を決める。通信方式には依存しない。

use protocol::{Emote, Expression, Gaze, Presence};

/// X は正面からの角度、Y は下限からの角度を 0.1 度単位で表す。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ActuatorTarget {
    pub x_tenth_deg: i16,
    pub y_tenth_deg: u16,
    pub rgb: [u8; 3],
}

impl ActuatorTarget {
    pub const NEUTRAL: Self = Self {
        x_tenth_deg: 0,
        y_tenth_deg: 450,
        rgb: [0; 3],
    };
}

/// 視線方向に小さく頭を向ける。Y は機構の推奨可動域 5..85 度から十分離す。
pub fn target(presence: Option<&Presence>, emote: Option<&Emote>) -> ActuatorTarget {
    let (gaze, expression, intensity) = if let Some(emote) = emote {
        (emote.gaze, emote.expression, emote.intensity)
    } else if let Some(presence) = presence {
        (presence.gaze, presence.expression, 35)
    } else {
        return ActuatorTarget::NEUTRAL;
    };

    let (x, y) = match gaze {
        Gaze::Center => (0, 0),
        Gaze::Left => (-100, 0),
        Gaze::Right => (100, 0),
        Gaze::Up => (0, -100),
        Gaze::Down => (0, 100),
        Gaze::Point { x, y } => (i32::from(x), i32::from(y)),
    };
    let strength = i32::from(intensity);
    let x_tenth_deg = (x * 300 * strength / 10_000) as i16;
    // 画面座標の +Y は下だが、M5 BSP のピッチ角は + が上になる。
    let y_tenth_deg = (450 - y * 150 * strength / 10_000) as u16;
    let palette: [u8; 3] = match expression {
        Expression::Happy | Expression::Grin | Expression::Playful => [255, 120, 12],
        Expression::Focused | Expression::Determined => [25, 90, 255],
        Expression::Sleepy | Expression::Calm => [80, 40, 170],
        Expression::Worried | Expression::Sad => [30, 90, 180],
        Expression::Surprised | Expression::Curious => [30, 200, 110],
        Expression::Wink => [220, 50, 130],
    };
    let mut rgb = [0; 3];
    for (out, component) in rgb.iter_mut().zip(palette) {
        // 常時点灯を控えめにし、Emote 強度 100 でも各チャンネル 1/4 以下とする。
        *out = (u16::from(component) * u16::from(intensity) / 400) as u8;
    }
    ActuatorTarget {
        x_tenth_deg,
        y_tenth_deg,
        rgb,
    }
}

/// 2 秒周期で 70–100% の範囲を往復させる。入力より明るくはしない。
pub fn breathing_rgb(base: [u8; 3], now_ms: u64) -> [u8; 3] {
    let phase = (now_ms % 2_000) as u16;
    let triangle = if phase < 1_000 { phase } else { 2_000 - phase };
    let scale = 70 + triangle * 30 / 1_000;
    base.map(|component| (u16::from(component) * scale / 100) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use protocol::EyeStyle;

    fn emote(gaze: Gaze, intensity: u8) -> Emote {
        Emote {
            expression: Expression::Curious,
            gaze,
            eyes: EyeStyle::Auto,
            intensity,
            duration_ms: 500,
        }
    }

    #[test]
    fn no_presence_is_neutral_and_zero_intensity_disables_motion_and_light() {
        assert_eq!(target(None, None), ActuatorTarget::NEUTRAL);
        assert_eq!(
            target(None, Some(&emote(Gaze::Right, 0))),
            ActuatorTarget::NEUTRAL
        );
    }

    #[test]
    fn motion_remains_inside_conservative_limits() {
        for x in [-100, 0, 100] {
            for y in [-100, 0, 100] {
                let output = target(None, Some(&emote(Gaze::Point { x, y }, 100)));
                assert!((-300..=300).contains(&output.x_tenth_deg));
                assert!((300..=600).contains(&output.y_tenth_deg));
                assert!(output.rgb.iter().all(|value| *value <= 63));
            }
        }
    }

    #[test]
    fn vertical_gaze_matches_the_physical_pitch_direction() {
        let up = target(None, Some(&emote(Gaze::Up, 100)));
        let center = target(None, Some(&emote(Gaze::Center, 100)));
        let down = target(None, Some(&emote(Gaze::Down, 100)));
        assert!(up.y_tenth_deg > center.y_tenth_deg);
        assert!(center.y_tenth_deg > down.y_tenth_deg);
    }

    #[test]
    fn emote_overrides_presence() {
        let presence = Presence {
            activity: None,
            detail: Default::default(),
            expression: Expression::Happy,
            gaze: Gaze::Left,
            eyes: EyeStyle::Auto,
            ttl_s: 0,
        };
        let output = target(Some(&presence), Some(&emote(Gaze::Right, 100)));
        assert_eq!(output.x_tenth_deg, 300);
        assert_eq!(output.y_tenth_deg, 450);
        assert_eq!(output.rgb, [7, 50, 27]);
        assert_eq!(target(Some(&presence), None).x_tenth_deg, -105);
    }

    #[test]
    fn breathing_never_exceeds_base_or_creates_light_when_disabled() {
        let base = [63, 27, 1];
        assert_eq!(breathing_rgb([0; 3], 500), [0; 3]);
        assert_eq!(breathing_rgb(base, 0), [44, 18, 0]);
        assert_eq!(breathing_rgb(base, 1_000), base);
        assert_eq!(breathing_rgb(base, 2_000), breathing_rgb(base, 0));
        for time in (0..=2_000).step_by(50) {
            let value = breathing_rgb(base, time);
            assert!(
                value
                    .iter()
                    .zip(base)
                    .all(|(actual, limit)| *actual <= limit)
            );
        }
    }
}
