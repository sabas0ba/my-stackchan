//! 所要時間を与えて残り時間を表示するタイマー。
//!
//! host にも外部の機器にも依存せず、デバイスの単調時計だけで動く。3D プリンタの進捗の
//! ように、認証情報なしには取得できない情報の代わりに、利用者が所要時間を与える用途を
//! 想定する。状態は保持せず、再起動で消える。
//!
//! 残りわずかと完了は、操作画面ではなく表情・首の動き・発光で知らせる。操作画面は顔を
//! 隠すため、知らせる用途には使わない。

use core::fmt::Write;

use protocol::{Card, Element, Emote, Expression, EyeStyle, Gaze, Row, Slot};

use super::{Panel, Plugin, fit};

const SECOND_MS: u64 = 1_000;
const MINUTE_MS: u64 = 60_000;
/// 設定できる時間の上限 (99 時間 59 分)。表示の桁数を固定するため。
const MAX_MINUTES: u64 = 99 * 60 + 59;
/// 操作の無い Panel を閉じるまでの時間。Panel は顔を隠すため、開いたままにしない。
const PANEL_IDLE_MS: u64 = 30_000;
const ADJUST_MINUTES: u64 = 10;

/// 「残りわずか」とする残り時間の上限。短いタイマーで開始の直後から知らせ続けないよう、
/// 全体の 1/5 と比べて短い方を用いる。
const ALMOST_MS: u64 = 5 * MINUTE_MS;
/// 「残りわずか」のうち、終わりの近くで知らせ方を強める残り時間の上限。
const FINAL_MS: u64 = MINUTE_MS;
/// 見回す間隔。動き続けると煩わしく、サーボの負荷にもなるため、間を空ける。
const ALMOST_GLANCE_EVERY_MS: u64 = 30 * SECOND_MS;
const FINAL_GLANCE_EVERY_MS: u64 = 10 * SECOND_MS;
/// 見回しの片側の時間。首は約 25°/秒でしか動かないため、向き切る時間を取る。
const GLANCE_SIDE_MS: u64 = 1_500;
/// 完了の直後に、うなずきを繰り返す時間と、上下の向きを保つ時間。
const CELEBRATE_MS: u64 = 15 * SECOND_MS;
const NOD_HALF_MS: u64 = 1_200;
/// 完了の表示を保つ時間の上限。利用者が席を外している場合を考え、タップされるまで保つが、
/// 消し忘れた表示を残し続けない。
const DONE_KEEP_MS: u64 = 30 * MINUTE_MS;
/// 合図の表情 (Emote) の長さと、出し直す間隔。長さを短くするのは、停止や補正で合図が
/// 不要になった後に表情が残る時間を抑えるためである。
const CUE_EMOTE_MS: u16 = 2_000;
const CUE_RENEW_MS: u64 = SECOND_MS;

const SETTING_BUTTONS: [&str; 6] = ["+1h", "+10m", "+1m", "CLEAR", "START", "CLOSE"];
const RUNNING_BUTTONS: [&str; 4] = ["+10m", "-10m", "STOP", "CLOSE"];

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
        finished_ms: u64,
    },
}

/// 表情・首の動き・発光による合図。首の向きと発光の強さは、どちらも `intensity` に
/// 比例する (behavior::target)。視線を正面にすれば、首を動かさずに発光だけを行える。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Cue {
    expression: Expression,
    eyes: EyeStyle,
    gaze: Gaze,
    intensity: u8,
}

#[derive(Default)]
pub struct Timer {
    state: State,
    emote: Option<Emote>,
    /// 最後に出した合図と、その時刻。同じ合図は `CUE_RENEW_MS` ごとに出し直す。
    last_cue: Option<(Cue, u64)>,
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

/// 左、右の順に見回す。`since_ms` は見回しの周期の始まりからの時間。見回していない間は
/// 正面を向き、`resting` の強さで発光だけを行う。
fn glance(expression: Expression, eyes: EyeStyle, since_ms: u64, resting: u8, moving: u8) -> Cue {
    let (gaze, intensity) = match since_ms / GLANCE_SIDE_MS {
        0 => (Gaze::Left, moving),
        1 => (Gaze::Right, moving),
        _ => (Gaze::Center, resting),
    };
    Cue {
        expression,
        eyes,
        gaze,
        intensity,
    }
}

impl Timer {
    fn remaining_ms(started_ms: u64, total_ms: u64, now_ms: u64) -> u64 {
        (started_ms + total_ms).saturating_sub(now_ms)
    }

    /// 現在の状態で出す合図。出さない場合は None。
    fn cue(&self, now_ms: u64) -> Option<Cue> {
        match self.state {
            State::Running {
                started_ms,
                total_ms,
                ..
            } => {
                let almost_ms = ALMOST_MS.min(total_ms / 5);
                let final_ms = FINAL_MS.min(almost_ms / 2);
                let remaining_ms = Self::remaining_ms(started_ms, total_ms, now_ms);
                if remaining_ms > almost_ms {
                    return None;
                }
                // 見回しの周期は、それぞれの段階に入った時点から数える。段階の始まりで
                // 1 回見回すことで、段階が変わったことが分かる。
                Some(if remaining_ms > final_ms {
                    glance(
                        Expression::Curious,
                        EyeStyle::Auto,
                        (almost_ms - remaining_ms) % ALMOST_GLANCE_EVERY_MS,
                        25,
                        70,
                    )
                } else {
                    glance(
                        Expression::Surprised,
                        EyeStyle::Wide,
                        (final_ms - remaining_ms) % FINAL_GLANCE_EVERY_MS,
                        50,
                        90,
                    )
                })
            }
            State::Done { finished_ms } => {
                let since_ms = now_ms.saturating_sub(finished_ms);
                Some(if since_ms < CELEBRATE_MS {
                    let up = (since_ms / NOD_HALF_MS).is_multiple_of(2);
                    Cue {
                        expression: Expression::Grin,
                        eyes: EyeStyle::Auto,
                        gaze: Gaze::Point {
                            x: 0,
                            y: if up { -100 } else { 100 },
                        },
                        intensity: 90,
                    }
                } else {
                    Cue {
                        expression: Expression::Happy,
                        eyes: EyeStyle::Auto,
                        gaze: Gaze::Center,
                        intensity: 20,
                    }
                })
            }
            State::Idle | State::Setting { .. } => None,
        }
    }

    /// 合図が変わった時と、同じ合図を出してから `CUE_RENEW_MS` が過ぎた時に Emote を出す。
    fn update_cue(&mut self, now_ms: u64) {
        let Some(cue) = self.cue(now_ms) else {
            self.last_cue = None;
            // 取り出される前の合図も捨てる。停止や補正の後に、古い合図が 1 回出るのを防ぐ。
            self.emote = None;
            return;
        };
        let due = self.last_cue.is_none_or(|(last, at_ms)| {
            last != cue || now_ms.saturating_sub(at_ms) >= CUE_RENEW_MS
        });
        if due {
            self.emote = Some(Emote {
                expression: cue.expression,
                gaze: cue.gaze,
                eyes: cue.eyes,
                intensity: cue.intensity,
                duration_ms: CUE_EMOTE_MS,
            });
            self.last_cue = Some((cue, now_ms));
        }
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
            } if Self::remaining_ms(started_ms, total_ms, now_ms) == 0 => State::Done {
                // 完了からの経過は、判定した時刻ではなく設定した終わりの時刻から数える。
                finished_ms: started_ms + total_ms,
            },
            State::Running {
                started_ms,
                total_ms,
                panel_until_ms: Some(until_ms),
            } if now_ms >= until_ms => State::Running {
                started_ms,
                total_ms,
                panel_until_ms: None,
            },
            State::Done { finished_ms } if now_ms >= finished_ms + DONE_KEEP_MS => State::Idle,
            state => state,
        };
        self.update_cue(now_ms);
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
            // 完了の表示は、Card のタップでも長押しでも消す。
            State::Done { .. } => State::Idle,
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
            state => state,
        };
    }

    fn card(&self, now_ms: u64) -> Option<Card> {
        let mut detail: heapless::String<{ protocol::MAX_CARD_TEXT_BYTES }> =
            heapless::String::new();
        let (ratio, status) = match self.state {
            State::Running {
                started_ms,
                total_ms,
                ..
            } => {
                let remaining_ms = Self::remaining_ms(started_ms, total_ms, now_ms);
                let _ = write!(
                    detail,
                    "total {}  tap to adjust",
                    minutes_label(total_ms / MINUTE_MS)
                );
                (
                    ((total_ms - remaining_ms) * 100 / total_ms) as u8,
                    remaining_label(remaining_ms),
                )
            }
            State::Done { finished_ms } => {
                // 席を外していた利用者が、いつ終わったかを分かるようにする。
                let _ = write!(
                    detail,
                    "{}m ago  tap to clear",
                    now_ms.saturating_sub(finished_ms) / MINUTE_MS
                );
                (100, fit("DONE"))
            }
            State::Idle | State::Setting { .. } => return None,
        };
        let rows = heapless::Vec::from_iter([
            Row {
                elements: heapless::Vec::from_iter([
                    Element::Bar {
                        ratio,
                        label: fit("TIMER"),
                    },
                    Element::Text { text: fit(&status) },
                ]),
                action: None,
            },
            Row {
                elements: heapless::Vec::from_iter([Element::Text { text: detail }]),
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
            // 完了は Panel で知らせない。Panel は顔を隠し、表情で知らせられなくなる。
            State::Idle | State::Done { .. } => None,
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
        }
    }

    fn take_emote(&mut self) -> Option<Emote> {
        self.emote.take()
    }
}

#[cfg(test)]
mod tests {
    use std::vec::Vec;

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

    /// 30 分のタイマー。「残りわずか」は残り 5 分から、その終わりの段階は残り 1 分から。
    fn thirty_minutes() -> Timer {
        started(&[1, 1, 1], 0)
    }

    fn glancing(timer: &Timer, now_ms: u64) -> (Expression, EyeStyle, Gaze, u8) {
        let cue = timer.cue(now_ms).expect("a cue is given");
        (cue.expression, cue.eyes, cue.gaze, cue.intensity)
    }

    /// 100 ms 刻みで進め、出された Emote を時刻とともに返す (firmware の呼出しの間隔)。
    fn run(timer: &mut Timer, from_ms: u64, to_ms: u64) -> Vec<(u64, Emote)> {
        let mut emotes = Vec::new();
        for now_ms in (from_ms..=to_ms).step_by(100) {
            timer.tick(now_ms);
            if let Some(emote) = timer.take_emote() {
                assert_eq!(emote.validate(), Ok(()), "{now_ms}");
                emotes.push((now_ms, emote));
            }
        }
        emotes
    }

    #[test]
    fn nothing_is_signalled_until_the_last_part_of_the_time() {
        let mut timer = thirty_minutes();
        assert!(run(&mut timer, 0, 25 * MINUTE_MS - 100).is_empty());
        assert_eq!(timer.cue(25 * MINUTE_MS - 1), None);
        assert!(timer.cue(25 * MINUTE_MS).is_some());
    }

    #[test]
    fn remaining_time_changes_the_face_and_glances_at_intervals() {
        let timer = thirty_minutes();
        let almost = 25 * MINUTE_MS;
        let curious = |gaze, intensity| (Expression::Curious, EyeStyle::Auto, gaze, intensity);
        // 段階に入った時に左、右の順に見回し、その後は正面を向いて弱く発光する。
        assert_eq!(glancing(&timer, almost), curious(Gaze::Left, 70));
        assert_eq!(glancing(&timer, almost + 1_500), curious(Gaze::Right, 70));
        assert_eq!(glancing(&timer, almost + 3_000), curious(Gaze::Center, 25));
        assert_eq!(glancing(&timer, almost + 29_999), curious(Gaze::Center, 25));
        assert_eq!(glancing(&timer, almost + 30_000), curious(Gaze::Left, 70));

        // 残り 1 分からは表情を強め、見回す間隔を詰める。
        let last = 29 * MINUTE_MS;
        let surprised = |gaze, intensity| (Expression::Surprised, EyeStyle::Wide, gaze, intensity);
        assert_eq!(glancing(&timer, last - 1).0, Expression::Curious);
        assert_eq!(glancing(&timer, last), surprised(Gaze::Left, 90));
        assert_eq!(glancing(&timer, last + 3_000), surprised(Gaze::Center, 50));
        assert_eq!(glancing(&timer, last + 10_000), surprised(Gaze::Left, 90));
    }

    #[test]
    fn short_timer_is_not_signalled_from_its_start() {
        // 1 分のタイマーでは、残り 12 秒 (全体の 1/5) から知らせる。
        let timer = started(&[2], 0);
        assert_eq!(timer.cue(0), None);
        assert_eq!(timer.cue(47_999), None);
        assert_eq!(glancing(&timer, 48_000).0, Expression::Curious);
        assert_eq!(glancing(&timer, 54_000).0, Expression::Surprised);
    }

    #[test]
    fn cue_is_emitted_on_change_and_renewed_every_second() {
        let mut timer = thirty_minutes();
        let almost = 25 * MINUTE_MS;
        let emotes = run(&mut timer, almost, almost + 5_000);
        let seen: Vec<(u64, Gaze)> = emotes
            .iter()
            .map(|(at_ms, emote)| (at_ms - almost, emote.gaze))
            .collect();
        assert_eq!(
            seen,
            [
                (0, Gaze::Left),
                (1_000, Gaze::Left),
                (1_500, Gaze::Right),
                (2_500, Gaze::Right),
                (3_000, Gaze::Center),
                (4_000, Gaze::Center),
                (5_000, Gaze::Center),
            ]
        );
        // 出し直す間隔より長く保ち、途切れさせない。
        assert!(
            emotes
                .iter()
                .all(|(_, emote)| u64::from(emote.duration_ms) > CUE_RENEW_MS)
        );
    }

    #[test]
    fn extending_or_stopping_ends_the_cue() {
        let mut timer = thirty_minutes();
        let now_ms = 26 * MINUTE_MS;
        assert!(!run(&mut timer, now_ms, now_ms).is_empty());
        timer.card_tap(now_ms);
        timer.button(0, now_ms); // +10m。残りは 14 分になる
        assert!(run(&mut timer, now_ms + 100, now_ms + 5_000).is_empty());

        let mut timer = thirty_minutes();
        timer.open(now_ms);
        timer.button(2, now_ms); // STOP
        assert!(run(&mut timer, now_ms, now_ms + 5_000).is_empty());
    }

    #[test]
    fn completion_nods_then_rests_and_keeps_the_face_visible() {
        let mut timer = started(&[2], 0); // 1 分
        timer.tick(MINUTE_MS - 100);
        assert_ne!(card_fields(&timer, MINUTE_MS - 100).unwrap().0, 100);

        timer.tick(MINUTE_MS);
        assert_eq!(card_fields(&timer, MINUTE_MS), Some((100, fit("DONE"))));
        assert_eq!(timer.panel(MINUTE_MS), None, "顔を隠さない");

        // 完了の直後は、上下にうなずきを繰り返す。
        let grin = |y| {
            (
                Expression::Grin,
                EyeStyle::Auto,
                Gaze::Point { x: 0, y },
                90,
            )
        };
        assert_eq!(glancing(&timer, MINUTE_MS), grin(-100));
        assert_eq!(glancing(&timer, MINUTE_MS + NOD_HALF_MS), grin(100));
        assert_eq!(glancing(&timer, MINUTE_MS + 2 * NOD_HALF_MS), grin(-100));
        // その後は首を動かさず、弱い発光と表情だけを保つ。
        assert_eq!(
            glancing(&timer, MINUTE_MS + CELEBRATE_MS),
            (Expression::Happy, EyeStyle::Auto, Gaze::Center, 20)
        );
        assert!(!run(&mut timer, 10 * MINUTE_MS, 10 * MINUTE_MS + 1_000).is_empty());
    }

    #[test]
    fn done_card_tells_how_long_ago_and_stays_until_dismissed_or_stale() {
        let detail = |timer: &Timer, now_ms: u64| match &timer.card(now_ms)?.rows[1].elements[0] {
            Element::Text { text } => Some(text.clone()),
            _ => None,
        };
        // 完了からの経過は、判定が遅れても設定した終わりの時刻から数える。
        let mut timer = started(&[2], 0);
        timer.tick(MINUTE_MS + 30_000);
        assert_eq!(
            detail(&timer, MINUTE_MS + 30_000),
            Some(fit("0m ago  tap to clear"))
        );
        assert_eq!(
            detail(&timer, 13 * MINUTE_MS),
            Some(fit("12m ago  tap to clear"))
        );
        timer
            .card(MINUTE_MS + DONE_KEEP_MS - 1)
            .unwrap()
            .validate()
            .expect("card fits the banner");

        timer.tick(MINUTE_MS + DONE_KEEP_MS - 100);
        assert!(timer.card(0).is_some(), "タップされるまで保つ");
        timer.tick(MINUTE_MS + DONE_KEEP_MS);
        assert_eq!(timer.card(0), None, "消し忘れは上限で消す");
        assert!(run(&mut timer, 40 * MINUTE_MS, 40 * MINUTE_MS + 2_000).is_empty());

        // Card のタップでも長押しでも消える。長押しで設定の Panel は開かない。
        for dismiss in [Timer::card_tap as fn(&mut Timer, u64), Timer::open] {
            let mut timer = started(&[2], 0);
            timer.tick(MINUTE_MS);
            dismiss(&mut timer, MINUTE_MS + 500);
            assert_eq!(timer.card(MINUTE_MS + 500), None);
            assert_eq!(timer.panel(MINUTE_MS + 500), None);
        }
    }

    #[test]
    fn completion_closes_the_adjust_panel() {
        let mut timer = started(&[2], 0);
        timer.card_tap(30_000);
        assert!(timer.panel(30_000).is_some());
        timer.tick(MINUTE_MS);
        assert_eq!(timer.panel(MINUTE_MS), None);
        assert_eq!(card_fields(&timer, MINUTE_MS), Some((100, fit("DONE"))));
    }
}
