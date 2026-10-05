//! 所要時間を与えて残り時間を表示するタイマー。
//!
//! host にも外部の機器にも依存せず、デバイスの単調時計だけで動く。3D プリンタの進捗の
//! ように、認証情報なしには取得できない情報の代わりに、利用者が所要時間を与える用途を
//! 想定する。状態は保持せず、再起動で消える。

use core::fmt::Write;

use protocol::{Card, Element, Emote, Expression, EyeStyle, Gaze, Row, Slot};

use super::{Panel, Plugin, fit};

const MINUTE_MS: u64 = 60_000;
/// 設定できる時間の上限 (99 時間 59 分)。表示の桁数を固定するため。
const MAX_MINUTES: u64 = 99 * 60 + 59;
/// 操作の無い Panel を閉じるまでの時間。Panel は顔を隠すため、開いたままにしない。
const PANEL_IDLE_MS: u64 = 30_000;
/// 完了の表示を保つ時間。
const DONE_MS: u64 = 60_000;
const ADJUST_MINUTES: u64 = 10;

const SETTING_BUTTONS: [&str; 6] = ["+1h", "+10m", "+1m", "CLEAR", "START", "CLOSE"];
const RUNNING_BUTTONS: [&str; 4] = ["+10m", "-10m", "STOP", "CLOSE"];
const DONE_BUTTONS: [&str; 1] = ["OK"];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Idle,
    Setting {
        minutes: u64,
        close_at_ms: u64,
    },
    Running {
        started_ms: u64,
        total_ms: u64,
        /// 補正の Panel を閉じる時刻。開いていない場合は None。
        panel_until_ms: Option<u64>,
    },
    Done {
        until_ms: u64,
    },
}

#[derive(Default)]
pub struct Timer {
    state: State,
    emote: Option<Emote>,
}

/// `1h05m` の形式。設定値と合計時間の表示に用いる。
fn minutes_label(minutes: u64) -> heapless::String<8> {
    let mut label = heapless::String::new();
    // 上限 (99h59m) までの値は容量に収まる。
    let _ = write!(label, "{}h{:02}m", minutes / 60, minutes % 60);
    label
}

/// `1:05:09` の形式。残り時間の表示に用いる。端数は切り上げ、0 になるのは完了時だけとする。
fn remaining_label(remaining_ms: u64) -> heapless::String<8> {
    let seconds = remaining_ms.div_ceil(1000);
    let mut label = heapless::String::new();
    let _ = write!(
        label,
        "{}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    );
    label
}

impl Timer {
    fn remaining_ms(started_ms: u64, total_ms: u64, now_ms: u64) -> u64 {
        (started_ms + total_ms).saturating_sub(now_ms)
    }

    fn setting_button(minutes: u64, index: usize, now_ms: u64) -> State {
        let setting = |minutes: u64| State::Setting {
            minutes: minutes.min(MAX_MINUTES),
            close_at_ms: now_ms + PANEL_IDLE_MS,
        };
        match index {
            0 => setting(minutes + 60),
            1 => setting(minutes + 10),
            2 => setting(minutes + 1),
            3 => setting(0),
            // 0 分では開始しない。押し間違いで即座に完了の表示が出るのを避ける。
            4 if minutes == 0 => setting(0),
            4 => State::Running {
                started_ms: now_ms,
                total_ms: minutes * MINUTE_MS,
                panel_until_ms: None,
            },
            5 => State::Idle,
            _ => setting(minutes),
        }
    }

    fn running_button(started_ms: u64, total_ms: u64, index: usize, now_ms: u64) -> State {
        let adjust = ADJUST_MINUTES * MINUTE_MS;
        let running = |total_ms: u64, open: bool| State::Running {
            started_ms,
            total_ms,
            panel_until_ms: open.then_some(now_ms + PANEL_IDLE_MS),
        };
        match index {
            0 => running((total_ms + adjust).min(MAX_MINUTES * MINUTE_MS), true),
            // 残りが補正の幅以下の場合は減らさない。押し間違いで完了させないため。
            1 if Self::remaining_ms(started_ms, total_ms, now_ms) > adjust => {
                running(total_ms - adjust, true)
            }
            2 => State::Idle,
            3 => running(total_ms, false),
            _ => running(total_ms, true),
        }
    }
}

impl Plugin for Timer {
    fn name(&self) -> &'static str {
        "TIMER"
    }

    fn tick(&mut self, now_ms: u64) {
        self.state = match self.state {
            State::Setting { close_at_ms, .. } if now_ms >= close_at_ms => State::Idle,
            State::Running {
                started_ms,
                total_ms,
                ..
            } if Self::remaining_ms(started_ms, total_ms, now_ms) == 0 => {
                // Panel が顔を隠すため、表情は首の動きと発光で完了を知らせる役割になる。
                self.emote = Some(Emote {
                    expression: Expression::Happy,
                    gaze: Gaze::Point { x: 0, y: -60 },
                    eyes: EyeStyle::Auto,
                    intensity: 80,
                    duration_ms: 10_000,
                });
                State::Done {
                    until_ms: now_ms + DONE_MS,
                }
            }
            State::Running {
                started_ms,
                total_ms,
                panel_until_ms: Some(until_ms),
            } if now_ms >= until_ms => State::Running {
                started_ms,
                total_ms,
                panel_until_ms: None,
            },
            State::Done { until_ms } if now_ms >= until_ms => State::Idle,
            state => state,
        };
    }

    fn open(&mut self, now_ms: u64) {
        self.state = match self.state {
            State::Idle => State::Setting {
                minutes: 0,
                close_at_ms: now_ms + PANEL_IDLE_MS,
            },
            State::Running {
                started_ms,
                total_ms,
                ..
            } => State::Running {
                started_ms,
                total_ms,
                panel_until_ms: Some(now_ms + PANEL_IDLE_MS),
            },
            state => state,
        };
    }

    fn card_tap(&mut self, now_ms: u64) {
        // 動作中の Card のタップは、長押しと同じく補正の Panel を開く。
        self.open(now_ms);
    }

    fn button(&mut self, index: usize, now_ms: u64) {
        self.state = match self.state {
            State::Setting { minutes, .. } => Self::setting_button(minutes, index, now_ms),
            State::Running {
                started_ms,
                total_ms,
                panel_until_ms: Some(_),
            } => Self::running_button(started_ms, total_ms, index, now_ms),
            State::Done { .. } => State::Idle,
            state => state,
        };
    }

    fn card(&self, now_ms: u64) -> Option<Card> {
        let State::Running {
            started_ms,
            total_ms,
            ..
        } = self.state
        else {
            return None;
        };
        let remaining_ms = Self::remaining_ms(started_ms, total_ms, now_ms);
        let ratio = ((total_ms - remaining_ms) * 100 / total_ms) as u8;
        let mut total: heapless::String<{ protocol::MAX_CARD_TEXT_BYTES }> =
            heapless::String::new();
        let _ = write!(
            total,
            "total {}  tap to adjust",
            minutes_label(total_ms / MINUTE_MS)
        );
        let rows = heapless::Vec::from_iter([
            Row {
                elements: heapless::Vec::from_iter([
                    Element::Bar {
                        ratio,
                        label: fit("TIMER"),
                    },
                    Element::Text {
                        text: fit(&remaining_label(remaining_ms)),
                    },
                ]),
                action: None,
            },
            Row {
                elements: heapless::Vec::from_iter([Element::Text { text: total }]),
                action: None,
            },
        ]);
        Some(Card {
            slot: Slot::BannerTop,
            ttl_s: 0,
            id: 0,
            rows,
            image: None,
        })
    }

    fn panel(&self, now_ms: u64) -> Option<Panel> {
        match self.state {
            State::Idle => None,
            State::Setting { minutes, .. } => Some(Panel::new(
                ["TIMER  set duration", minutes_label(minutes).as_str()],
                &SETTING_BUTTONS,
            )),
            State::Running {
                started_ms,
                total_ms,
                panel_until_ms,
            } => panel_until_ms.map(|_| {
                let mut line: heapless::String<{ super::PANEL_LINE_BYTES }> =
                    heapless::String::new();
                let _ = write!(
                    line,
                    "{} left of {}",
                    remaining_label(Self::remaining_ms(started_ms, total_ms, now_ms)),
                    minutes_label(total_ms / MINUTE_MS)
                );
                Panel::new(["TIMER  running", line.as_str()], &RUNNING_BUTTONS)
            }),
            State::Done { .. } => Some(Panel::new(["TIMER", "DONE"], &DONE_BUTTONS)),
        }
    }

    fn take_emote(&mut self) -> Option<Emote> {
        self.emote.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: usize = 4;

    /// 長押しから `minutes` 分を設定して開始する。開始の時刻は `now_ms`。
    fn started(minutes: &[usize], now_ms: u64) -> Timer {
        let mut timer = Timer::default();
        timer.open(now_ms);
        for button in minutes {
            timer.button(*button, now_ms);
        }
        timer.button(START, now_ms);
        timer
    }

    fn lines(timer: &Timer, now_ms: u64) -> Option<(heapless::String<30>, heapless::String<30>)> {
        timer
            .panel(now_ms)
            .map(|panel| (panel.lines[0].clone(), panel.lines[1].clone()))
    }

    fn card_fields(timer: &Timer, now_ms: u64) -> Option<(u8, heapless::String<48>)> {
        let card = timer.card(now_ms)?;
        card.validate().expect("card fits the banner");
        match (&card.rows[0].elements[0], &card.rows[0].elements[1]) {
            (Element::Bar { ratio, .. }, Element::Text { text }) => Some((*ratio, text.clone())),
            _ => None,
        }
    }

    #[test]
    fn duration_is_built_from_buttons_and_capped() {
        let mut timer = Timer::default();
        assert_eq!(timer.panel(0), None);
        timer.open(0);
        assert_eq!(lines(&timer, 0).unwrap().1, "0h00m");
        for button in [0, 0, 0, 1, 1, 2, 2, 2, 2] {
            timer.button(button, 0);
        }
        assert_eq!(lines(&timer, 0).unwrap().1, "3h24m");
        timer.button(3, 0);
        assert_eq!(lines(&timer, 0).unwrap().1, "0h00m");
        for _ in 0..120 {
            timer.button(0, 0);
        }
        assert_eq!(lines(&timer, 0).unwrap().1, "99h59m");
        assert_eq!(timer.card(0), None, "設定中は帯に出さない");
    }

    #[test]
    fn start_requires_a_duration_and_close_discards_the_setting() {
        let mut timer = Timer::default();
        timer.open(0);
        timer.button(START, 0);
        assert!(timer.panel(0).is_some(), "0 分では開始しない");
        assert_eq!(timer.card(0), None);
        timer.button(2, 0);
        timer.button(5, 0);
        assert_eq!(timer.panel(0), None);
        timer.open(0);
        assert_eq!(lines(&timer, 0).unwrap().1, "0h00m", "前の設定は残らない");
    }

    #[test]
    fn idle_setting_panel_closes_and_input_extends_it() {
        let mut timer = Timer::default();
        timer.open(0);
        timer.tick(PANEL_IDLE_MS - 1);
        assert!(timer.panel(0).is_some());
        timer.button(2, PANEL_IDLE_MS - 1);
        timer.tick(PANEL_IDLE_MS);
        assert!(timer.panel(0).is_some(), "操作から数え直す");
        timer.tick(2 * PANEL_IDLE_MS - 1);
        assert_eq!(timer.panel(0), None);
    }

    #[test]
    fn running_card_counts_down_from_the_monotonic_clock() {
        // 1 時間 30 分。開始の時刻が 0 でなくても、そこからの経過で数える。
        let timer = started(&[0, 1, 1, 1], 5_000);
        assert_eq!(timer.panel(5_000), None, "開始すると Panel を閉じる");
        assert_eq!(card_fields(&timer, 5_000), Some((0, fit("1:30:00"))));
        assert_eq!(card_fields(&timer, 5_001), Some((0, fit("1:30:00"))));
        assert_eq!(card_fields(&timer, 6_000), Some((0, fit("1:29:59"))));
        assert_eq!(
            card_fields(&timer, 5_000 + 45 * MINUTE_MS),
            Some((50, fit("0:45:00")))
        );
        assert_eq!(
            card_fields(&timer, 5_000 + 90 * MINUTE_MS - 1),
            Some((99, fit("0:00:01")))
        );
    }

    #[test]
    fn running_timer_is_adjusted_and_stopped_from_its_panel() {
        let mut timer = started(&[1, 1, 1], 0); // 30 分
        timer.card_tap(MINUTE_MS);
        assert_eq!(lines(&timer, MINUTE_MS).unwrap().1, "0:29:00 left of 0h30m");
        timer.button(0, MINUTE_MS);
        assert_eq!(card_fields(&timer, MINUTE_MS), Some((2, fit("0:39:00"))));
        timer.button(1, MINUTE_MS);
        timer.button(1, MINUTE_MS);
        assert_eq!(card_fields(&timer, MINUTE_MS), Some((5, fit("0:19:00"))));
        // 残りが 10 分以下では減らさない。
        timer.button(1, 11 * MINUTE_MS);
        assert_eq!(
            card_fields(&timer, 11 * MINUTE_MS),
            Some((55, fit("0:09:00")))
        );
        assert!(timer.panel(11 * MINUTE_MS).is_some());

        timer.button(3, 11 * MINUTE_MS);
        assert_eq!(timer.panel(11 * MINUTE_MS), None, "CLOSE は動作を続ける");
        assert!(timer.card(11 * MINUTE_MS).is_some());

        // Panel を閉じた後のボタンの番号は無視する (表示と操作の食い違いを避ける)。
        timer.button(2, 11 * MINUTE_MS);
        assert!(timer.card(11 * MINUTE_MS).is_some());

        timer.open(12 * MINUTE_MS);
        timer.button(2, 12 * MINUTE_MS);
        assert_eq!(timer.card(12 * MINUTE_MS), None, "STOP で停止する");
        assert_eq!(timer.panel(12 * MINUTE_MS), None);
    }

    #[test]
    fn adjustment_is_capped_and_idle_panel_closes_without_stopping() {
        let mut timer = Timer::default();
        timer.open(0);
        for _ in 0..120 {
            timer.button(0, 0);
        }
        timer.button(START, 0);
        timer.card_tap(0);
        timer.button(0, 0);
        assert_eq!(card_fields(&timer, 0), Some((0, fit("99:59:00"))));
        timer.tick(PANEL_IDLE_MS);
        assert_eq!(timer.panel(PANEL_IDLE_MS), None);
        assert!(timer.card(PANEL_IDLE_MS).is_some());
    }

    #[test]
    fn completion_shows_done_requests_an_emote_once_and_times_out() {
        let mut timer = started(&[2], 0); // 1 分
        timer.tick(MINUTE_MS - 1);
        assert!(timer.card(MINUTE_MS - 1).is_some());
        assert_eq!(timer.take_emote(), None);

        timer.tick(MINUTE_MS);
        assert_eq!(timer.card(MINUTE_MS), None);
        assert_eq!(lines(&timer, MINUTE_MS).unwrap().1, "DONE");
        let emote = timer.take_emote().expect("completion requests an emote");
        assert_eq!(emote.validate(), Ok(()));
        assert_eq!(timer.take_emote(), None);

        timer.tick(MINUTE_MS + DONE_MS - 1);
        assert!(timer.panel(0).is_some());
        timer.tick(MINUTE_MS + DONE_MS);
        assert_eq!(timer.panel(0), None);

        // 完了の表示は、どのボタンでも閉じる。長押しでは閉じない。
        let mut timer = started(&[2], 0);
        timer.tick(MINUTE_MS);
        timer.open(MINUTE_MS);
        assert!(timer.panel(MINUTE_MS).is_some());
        timer.button(0, MINUTE_MS);
        assert_eq!(timer.panel(MINUTE_MS), None);
    }

    #[test]
    fn completion_is_reached_while_the_adjust_panel_is_open() {
        let mut timer = started(&[2], 0);
        timer.card_tap(30_000);
        timer.tick(MINUTE_MS);
        assert_eq!(lines(&timer, MINUTE_MS).unwrap().1, "DONE");
    }
}
