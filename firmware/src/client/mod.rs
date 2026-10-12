//! Client 側の plugin。firmware に構築時に組み込み、host が無くても動く機能を追加する。
//!
//! plugin は画素を描かない。host が送るものと同じ宣言的な内容 (Card) と、操作画面
//! (Panel) を出力し、描画は renderer が行う。描画の経路を 1 本に保ち、host 上の試験で
//! 表示を確かめられるようにするためである。
//!
//! plugin は firmware と同じアドレス空間で動き、隔離できない。firmware の一部として
//! 信頼するコードだけを登録する。設計は docs/plugin.md の「Client 側の plugin」を参照する。

pub mod timer;

use embedded_graphics::{prelude::*, primitives::Rectangle};
use protocol::{Card, Emote};

pub const PANEL_LINES: usize = 2;
/// Panel の文字の行の長さ。10 px の文字で画面の幅 (左右 10 px の余白を除く) に収まる。
pub const PANEL_LINE_BYTES: usize = 30;
pub const PANEL_BUTTONS: usize = 6;
/// ボタンのラベルの長さ。10 px の文字でボタンの幅に収まる。
pub const BUTTON_LABEL_BYTES: usize = 9;
/// 登録できる plugin の数。一覧の Panel に 1 画面で並ぶ数とする。
pub const MAX_PLUGINS: usize = PANEL_BUTTONS;

/// plugin の一覧の Panel と、複数の Card の巡回の間隔。
const LAUNCHER_IDLE_MS: u64 = 30_000;
const CARD_ROTATE_MS: u64 = 5_000;

const BUTTON_COLUMNS: usize = 3;
const BUTTONS_TOP: i32 = 64;
const BUTTON_CELL: Size = Size::new(106, 88);
/// ボタンの枠と、隣のボタンとの間に空ける幅。隣のボタンの誤操作を減らす。
const BUTTON_INSET: i32 = 4;

pub type Line = heapless::String<PANEL_LINE_BYTES>;
pub type Label = heapless::String<BUTTON_LABEL_BYTES>;

/// 上限まで文字を入れる。収まらない分は捨てる (表示が欠けるだけで、動作は続ける)。
pub fn fit<const N: usize>(text: &str) -> heapless::String<N> {
    let mut fitted = heapless::String::new();
    for ch in text.chars() {
        if fitted.push(ch).is_err() {
            break;
        }
    }
    fitted
}

/// Overlay に出す操作画面。文字の行とボタンからなる。
///
/// Card と別に設けるのは、Card の行 (高さ 20 px) が指での操作に小さすぎるためである。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Panel {
    pub lines: [Line; PANEL_LINES],
    pub buttons: heapless::Vec<Label, PANEL_BUTTONS>,
}

impl Panel {
    pub fn new(lines: [&str; PANEL_LINES], buttons: &[&str]) -> Self {
        Self {
            lines: lines.map(fit),
            buttons: buttons
                .iter()
                .take(PANEL_BUTTONS)
                .map(|label| fit(label))
                .collect(),
        }
    }
}

/// `index` 番目のボタンの枠。3 列に左上から並べる。描画とタップの照合で共用する。
pub fn button_frame(index: usize) -> Rectangle {
    let column = (index % BUTTON_COLUMNS) as i32;
    let row = (index / BUTTON_COLUMNS) as i32;
    let cell = Point::new(
        1 + column * BUTTON_CELL.width as i32,
        BUTTONS_TOP + row * BUTTON_CELL.height as i32,
    );
    Rectangle::new(
        cell + Point::new(BUTTON_INSET, BUTTON_INSET),
        BUTTON_CELL - Size::new(2 * BUTTON_INSET as u32, 2 * BUTTON_INSET as u32),
    )
}

/// タップ位置にあるボタンの番号。枠の外は None。
pub fn button_at(x: u16, y: u16, count: usize) -> Option<usize> {
    let point = Point::new(i32::from(x), i32::from(y));
    (0..count.min(PANEL_BUTTONS)).find(|index| button_frame(*index).contains(point))
}

/// Client 側の plugin。時刻は単調時計 (ms) で受け取る。
pub trait Plugin {
    /// 一覧の Panel に出す名前。
    fn name(&self) -> &'static str;
    /// 周期的に呼ばれる。期限の判定に用いる。
    fn tick(&mut self, now_ms: u64);
    /// 長押しから開かれた。
    fn open(&mut self, now_ms: u64);
    /// 帯に出している自身の Card がタップされた。
    fn card_tap(&mut self, now_ms: u64);
    /// 自身の Panel のボタンが押された。
    fn button(&mut self, index: usize, now_ms: u64);
    /// 帯に出す Card。出さない場合は None。
    fn card(&self, now_ms: u64) -> Option<Card>;
    /// Overlay に出す Panel。出さない場合は None。
    fn panel(&self, now_ms: u64) -> Option<Panel>;
    /// 一時的な表情の要求を取り出す。
    fn take_emote(&mut self) -> Option<Emote>;
}

/// 登録された plugin の一覧。構築時に確定する。
pub trait Registry {
    fn plugins(&self) -> heapless::Vec<&dyn Plugin, MAX_PLUGINS>;
    fn plugins_mut(&mut self) -> heapless::Vec<&mut dyn Plugin, MAX_PLUGINS>;
}

/// firmware に組み込む plugin。追加する場合は、フィールドと下の 2 つの一覧に加える。
#[derive(Default)]
pub struct Builtin {
    timer: timer::Timer,
}

impl Registry for Builtin {
    fn plugins(&self) -> heapless::Vec<&dyn Plugin, MAX_PLUGINS> {
        heapless::Vec::from_iter([&self.timer as &dyn Plugin])
    }

    fn plugins_mut(&mut self) -> heapless::Vec<&mut dyn Plugin, MAX_PLUGINS> {
        heapless::Vec::from_iter([&mut self.timer as &mut dyn Plugin])
    }
}

pub type Clients = Manager<Builtin>;

/// Panel を出しているもの。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    /// plugin の一覧。
    Launcher,
    Plugin(usize),
}

/// plugin が現在求めている表示。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View {
    /// 帯に出す Card と、それを出している plugin。
    pub card: Option<(usize, Card)>,
    pub panel: Option<(Owner, Panel)>,
}

/// 複数の plugin の表示と入力を配分する。
#[derive(Default)]
pub struct Manager<R> {
    registry: R,
    /// 一覧の Panel を閉じる時刻。開いていない場合は None。
    launcher_until_ms: Option<u64>,
}

impl<R: Registry> Manager<R> {
    pub fn new(registry: R) -> Self {
        Self {
            registry,
            launcher_until_ms: None,
        }
    }

    pub fn tick(&mut self, now_ms: u64) {
        for plugin in self.registry.plugins_mut() {
            plugin.tick(now_ms);
        }
        if self.launcher_until_ms.is_some_and(|until| now_ms >= until) {
            self.launcher_until_ms = None;
        }
    }

    /// 長押し。plugin が 1 つなら直接開き、複数なら一覧を出す。Panel が開いている間は
    /// 何もしない (操作中の Panel を別のもので置き換えないため)。
    pub fn open(&mut self, now_ms: u64) {
        if self.view(now_ms).panel.is_some() {
            return;
        }
        let mut plugins = self.registry.plugins_mut();
        match plugins.len() {
            0 => {}
            1 => plugins[0].open(now_ms),
            _ => self.launcher_until_ms = Some(now_ms + LAUNCHER_IDLE_MS),
        }
    }

    pub fn card_tap(&mut self, plugin: usize, now_ms: u64) {
        if let Some(plugin) = self.registry.plugins_mut().get_mut(plugin) {
            plugin.card_tap(now_ms);
        }
    }

    pub fn button(&mut self, owner: Owner, index: usize, now_ms: u64) {
        let mut plugins = self.registry.plugins_mut();
        match owner {
            Owner::Launcher => {
                self.launcher_until_ms = None;
                // plugin の名前の後ろに置いた CLOSE では、閉じるだけにする。
                if let Some(plugin) = plugins.get_mut(index) {
                    plugin.open(now_ms);
                }
            }
            Owner::Plugin(plugin) => {
                if let Some(plugin) = plugins.get_mut(plugin) {
                    plugin.button(index, now_ms);
                }
            }
        }
    }

    pub fn take_emote(&mut self) -> Option<Emote> {
        self.registry
            .plugins_mut()
            .into_iter()
            .find_map(|plugin| plugin.take_emote())
    }

    pub fn view(&self, now_ms: u64) -> View {
        let plugins = self.registry.plugins();
        let panel = if self.launcher_until_ms.is_some() {
            let mut buttons: heapless::Vec<Label, PANEL_BUTTONS> =
                plugins.iter().map(|plugin| fit(plugin.name())).collect();
            // 一覧が上限まで埋まっている場合は置けない。その場合は時間切れで閉じる。
            let _ = buttons.push(fit("CLOSE"));
            Some((
                Owner::Launcher,
                Panel {
                    lines: [fit("PLUGINS"), Line::new()],
                    buttons,
                },
            ))
        } else {
            plugins
                .iter()
                .enumerate()
                .find_map(|(index, plugin)| Some((Owner::Plugin(index), plugin.panel(now_ms)?)))
        };
        // 帯は 1 本だけを使うため、Card を出す plugin が複数ある場合は順に出す。
        let with_card: heapless::Vec<usize, MAX_PLUGINS> = plugins
            .iter()
            .enumerate()
            .filter(|(_, plugin)| plugin.card(now_ms).is_some())
            .map(|(index, _)| index)
            .collect();
        let card = (!with_card.is_empty()).then(|| {
            let turn = (now_ms / CARD_ROTATE_MS) as usize % with_card.len();
            let index = with_card[turn];
            (
                index,
                plugins[index].card(now_ms).expect("card was present"),
            )
        });
        View { card, panel }
    }
}

#[cfg(test)]
mod tests {
    use core::cell::RefCell;
    use std::{string::String, vec, vec::Vec};

    use super::*;

    /// 呼出しを記録し、指定された表示を返す plugin。
    #[derive(Default)]
    struct Fake {
        name: &'static str,
        card: Option<&'static str>,
        panel: Option<&'static str>,
        calls: RefCell<Vec<String>>,
    }

    impl Fake {
        fn calls(&self) -> Vec<String> {
            self.calls.take()
        }
    }

    impl Plugin for Fake {
        fn name(&self) -> &'static str {
            self.name
        }
        fn tick(&mut self, _now_ms: u64) {}
        fn open(&mut self, _now_ms: u64) {
            self.calls.get_mut().push("open".into());
        }
        fn card_tap(&mut self, _now_ms: u64) {
            self.calls.get_mut().push("card_tap".into());
        }
        fn button(&mut self, index: usize, _now_ms: u64) {
            self.calls.get_mut().push(std::format!("button {index}"));
        }
        fn card(&self, _now_ms: u64) -> Option<Card> {
            let text = self.card?;
            let mut rows = heapless::Vec::new();
            rows.push(protocol::Row {
                elements: heapless::Vec::from_iter([protocol::Element::Text { text: fit(text) }]),
                action: None,
            })
            .unwrap();
            Some(Card {
                slot: protocol::Slot::BannerTop,
                ttl_s: 0,
                id: 0,
                rows,
                image: None,
            })
        }
        fn panel(&self, _now_ms: u64) -> Option<Panel> {
            Some(Panel::new([self.panel?, ""], &["A", "B"]))
        }
        fn take_emote(&mut self) -> Option<Emote> {
            None
        }
    }

    #[derive(Default)]
    struct Fakes(Vec<Fake>);

    impl Registry for Fakes {
        fn plugins(&self) -> heapless::Vec<&dyn Plugin, MAX_PLUGINS> {
            self.0.iter().map(|fake| fake as &dyn Plugin).collect()
        }
        fn plugins_mut(&mut self) -> heapless::Vec<&mut dyn Plugin, MAX_PLUGINS> {
            self.0
                .iter_mut()
                .map(|fake| fake as &mut dyn Plugin)
                .collect()
        }
    }

    fn fake(name: &'static str) -> Fake {
        Fake {
            name,
            ..Fake::default()
        }
    }

    fn card_text(view: &View) -> Option<(usize, String)> {
        let (index, card) = view.card.as_ref()?;
        match &card.rows[0].elements[0] {
            protocol::Element::Text { text } => Some((*index, text.as_str().into())),
            _ => None,
        }
    }

    #[test]
    fn buttons_are_hit_inside_their_frames_only() {
        // 3 列 × 2 行。枠は画面内に収まり、互いに重ならない。
        let frames: Vec<Rectangle> = (0..PANEL_BUTTONS).map(button_frame).collect();
        let screen = Rectangle::new(Point::zero(), Size::new(320, 240));
        for (index, frame) in frames.iter().enumerate() {
            assert_eq!(frame.intersection(&screen), *frame, "{index}");
            for other in &frames[index + 1..] {
                assert!(frame.intersection(other).is_zero_sized(), "{index}");
            }
            let center = frame.center();
            assert_eq!(
                button_at(center.x as u16, center.y as u16, PANEL_BUTTONS),
                Some(index)
            );
        }
        assert_eq!(frames[0].top_left, Point::new(5, 68));
        assert_eq!(frames[0].size, Size::new(98, 80));
        assert_eq!(frames[4].top_left, Point::new(111, 156));
        // 文字の行、ボタンの間、存在しないボタンの位置では何も選ばない。
        assert_eq!(button_at(160, 30, PANEL_BUTTONS), None);
        assert_eq!(button_at(107, 100, PANEL_BUTTONS), None);
        assert_eq!(button_at(160, 200, 4), None);
    }

    #[test]
    fn long_text_is_cut_to_the_capacity() {
        let panel = Panel::new(
            ["0123456789012345678901234567890123", ""],
            &["0123456789", "a", "b", "c", "d", "e", "f"],
        );
        assert_eq!(panel.lines[0].len(), PANEL_LINE_BYTES);
        assert_eq!(panel.buttons.len(), PANEL_BUTTONS);
        assert_eq!(panel.buttons[0], "012345678");
    }

    #[test]
    fn single_plugin_opens_directly_and_receives_its_input() {
        let mut manager = Manager::new(Fakes(vec![fake("ONE")]));
        manager.open(0);
        assert_eq!(manager.registry.0[0].calls(), ["open"]);
        assert_eq!(manager.view(0), View::default());

        manager.registry.0[0].panel = Some("panel");
        manager.registry.0[0].card = Some("card");
        let view = manager.view(0);
        assert_eq!(view.panel.as_ref().unwrap().0, Owner::Plugin(0));
        assert_eq!(card_text(&view), Some((0, "card".into())));

        // Panel が開いている間の長押しは、Panel を置き換えない。
        manager.open(0);
        manager.button(Owner::Plugin(0), 1, 0);
        manager.card_tap(0, 0);
        assert_eq!(manager.registry.0[0].calls(), ["button 1", "card_tap"]);
    }

    #[test]
    fn several_plugins_are_chosen_from_a_launcher_that_times_out() {
        let mut manager = Manager::new(Fakes(vec![fake("ONE"), fake("TWO")]));
        manager.open(1_000);
        let (owner, panel) = manager.view(1_000).panel.unwrap();
        assert_eq!(owner, Owner::Launcher);
        assert_eq!(panel.buttons, ["ONE", "TWO", "CLOSE"]);

        manager.button(Owner::Launcher, 1, 2_000);
        assert_eq!(manager.registry.0[1].calls(), ["open"]);
        assert!(manager.registry.0[0].calls().is_empty());
        assert_eq!(manager.view(2_000).panel, None);

        // CLOSE は閉じるだけで、どの plugin も開かない。
        manager.open(3_000);
        manager.button(Owner::Launcher, 2, 3_000);
        assert_eq!(manager.view(3_000).panel, None);
        assert!(
            manager
                .registry
                .0
                .iter()
                .all(|fake| fake.calls().is_empty())
        );

        manager.open(10_000);
        manager.tick(10_000 + LAUNCHER_IDLE_MS - 1);
        assert!(manager.view(0).panel.is_some());
        manager.tick(10_000 + LAUNCHER_IDLE_MS);
        assert_eq!(manager.view(0).panel, None);
    }

    #[test]
    fn cards_of_several_plugins_take_turns() {
        let mut manager = Manager::new(Fakes(vec![fake("ONE"), fake("TWO"), fake("THREE")]));
        manager.registry.0[0].card = Some("first");
        manager.registry.0[2].card = Some("third");
        assert_eq!(card_text(&manager.view(0)), Some((0, "first".into())));
        assert_eq!(
            card_text(&manager.view(CARD_ROTATE_MS)),
            Some((2, "third".into()))
        );
        assert_eq!(
            card_text(&manager.view(2 * CARD_ROTATE_MS)),
            Some((0, "first".into()))
        );
    }
}
