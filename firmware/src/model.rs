//! Slot の内容と単調時計に基づく表示期限。描画の成功後に状態を確定する。

use embedded_graphics::{pixelcolor::Rgb565, prelude::DrawTarget};
use protocol::{
    Card, Emote, Event, Expression, EyeStyle, Gaze, ImageBegin, InputMode, MAX_IMAGE_REGION_BYTES,
    MAX_TEXT_BYTES, Message, Presence, Reply, Slot,
};

/// タップ位置の照合に使う配置。renderer の描画位置と一致させる。
const BANNER_HEIGHT: u16 = 48;
const BOTTOM_BANNER_Y: u16 = 192;
const CARD_TOP_MARGIN: u16 = 4;

const DEMO_FACES: [(Expression, &str); 12] = [
    (Expression::Happy, "01/12 HAPPY"),
    (Expression::Focused, "02/12 FOCUSED"),
    (Expression::Sleepy, "03/12 SLEEPY"),
    (Expression::Worried, "04/12 WORRIED"),
    (Expression::Surprised, "05/12 SURPRISED"),
    (Expression::Grin, "06/12 GRIN"),
    (Expression::Calm, "07/12 CALM"),
    (Expression::Curious, "08/12 CURIOUS"),
    (Expression::Playful, "09/12 PLAYFUL"),
    (Expression::Wink, "10/12 WINK"),
    (Expression::Sad, "11/12 SAD"),
    (Expression::Determined, "12/12 DETERMINED"),
];

pub const DEMO_FACE_COUNT: usize = DEMO_FACES.len();

#[derive(Clone, Debug, PartialEq, Eq)]
// firmware では動的確保を使わず、固定長の Card を Slot の状態に保持する。
#[allow(clippy::large_enum_variant)]
pub enum Content {
    Text(heapless::String<MAX_TEXT_BYTES>),
    Card(Card),
    /// 画像領域。画素は Controller の ImagePixels にあり、表示状態には領域だけを持つ。
    Image(ImageRegion),
}

/// 画面上の画像領域。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageRegion {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

/// 受信中の画像。行は上から順に受け取る。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PendingImage {
    begin: ImageBegin,
    next_row: u16,
}

/// 画像領域の画素。最大の領域 (160x120) の 1 枚分を static に持ち、動的確保を行わない。
/// 受信した行は表示中の画像と同じ領域に書くため、受信の途中で再描画が起きると部分的に
/// 更新された画像が見え得る (低い更新頻度を前提とし、許容する)。
pub struct ImagePixels {
    pixels: [u8; MAX_IMAGE_REGION_BYTES],
}

impl Default for ImagePixels {
    fn default() -> Self {
        Self {
            pixels: [0; MAX_IMAGE_REGION_BYTES],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub content: Content,
    deadline_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PresenceEntry {
    value: Presence,
    deadline_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct EmoteEntry {
    value: Emote,
    deadline_ms: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisplayState {
    slots: [Option<Entry>; 3],
    presence: Option<PresenceEntry>,
    emote: Option<EmoteEntry>,
}

impl DisplayState {
    fn can_blink(&self) -> bool {
        self.get(Slot::Overlay).is_none() && self.active_eyes() == EyeStyle::Auto
    }

    pub fn get(&self, slot: Slot) -> Option<&Entry> {
        self.slots[slot_index(slot)].as_ref()
    }

    pub fn presence(&self) -> Option<&Presence> {
        self.presence.as_ref().map(|entry| &entry.value)
    }

    pub fn emote(&self) -> Option<&Emote> {
        self.emote.as_ref().map(|entry| &entry.value)
    }

    fn active_eyes(&self) -> EyeStyle {
        self.emote()
            .map(|emote| emote.eyes)
            .or_else(|| self.presence().map(|presence| presence.eyes))
            .unwrap_or(EyeStyle::Auto)
    }

    fn set(&mut self, slot: Slot, content: Content, ttl_s: u16, now_ms: u64) {
        self.slots[slot_index(slot)] = Some(Entry {
            content,
            deadline_ms: (ttl_s != 0).then(|| now_ms.saturating_add(u64::from(ttl_s) * 1000)),
        });
    }

    fn set_presence(&mut self, value: Presence, now_ms: u64) {
        self.presence = Some(PresenceEntry {
            deadline_ms: (value.ttl_s != 0)
                .then(|| now_ms.saturating_add(u64::from(value.ttl_s) * 1000)),
            value,
        });
    }

    fn set_emote(&mut self, value: Emote, now_ms: u64) {
        self.emote = Some(EmoteEntry {
            deadline_ms: now_ms.saturating_add(u64::from(value.duration_ms)),
            value,
        });
    }

    fn has_expired(&self, now_ms: u64) -> bool {
        self.slots
            .iter()
            .flatten()
            .any(|entry| entry.deadline_ms.is_some_and(|deadline| now_ms >= deadline))
            || self
                .presence
                .as_ref()
                .is_some_and(|entry| entry.deadline_ms.is_some_and(|deadline| now_ms >= deadline))
            || self
                .emote
                .as_ref()
                .is_some_and(|entry| now_ms >= entry.deadline_ms)
    }

    fn expire(&mut self, now_ms: u64) {
        for slot in &mut self.slots {
            if slot
                .as_ref()
                .is_some_and(|entry| entry.deadline_ms.is_some_and(|deadline| now_ms >= deadline))
            {
                *slot = None;
            }
        }
        if self
            .presence
            .as_ref()
            .is_some_and(|entry| entry.deadline_ms.is_some_and(|deadline| now_ms >= deadline))
        {
            self.presence = None;
        }
        if self
            .emote
            .as_ref()
            .is_some_and(|entry| now_ms >= entry.deadline_ms)
        {
            self.emote = None;
        }
    }
}

fn slot_index(slot: Slot) -> usize {
    match slot {
        Slot::BannerTop => 0,
        Slot::BannerBottom => 1,
        Slot::Overlay => 2,
    }
}

pub struct Controller {
    state: DisplayState,
    seq: u32,
    pitch_trim_raw_steps: i16,
    input_mode: InputMode,
    demo_index: Option<usize>,
    blink: bool,
    startup: bool,
    image: ImagePixels,
    pending_image: Option<PendingImage>,
}

impl Default for Controller {
    fn default() -> Self {
        Self {
            state: DisplayState::default(),
            seq: 0,
            pitch_trim_raw_steps: 0,
            input_mode: InputMode::Demo,
            demo_index: None,
            blink: false,
            startup: true,
            image: ImagePixels::default(),
            pending_image: None,
        }
    }
}

// 固定周期内の間隔を変え、通信が途絶えても顔が静止し続けないようにする。
fn blink_at(now_ms: u64) -> bool {
    let phase = now_ms % 15_700;
    (4_000..4_120).contains(&phase)
        || (9_500..9_620).contains(&phase)
        || (9_790..9_910).contains(&phase)
        || (14_000..14_120).contains(&phase)
}

#[derive(Debug, PartialEq, Eq)]
pub enum HandleError<E> {
    InvalidCard,
    InvalidFace,
    InvalidImage,
    Draw(E),
}

impl Controller {
    pub fn pitch_trim_raw_steps(&self) -> i16 {
        self.pitch_trim_raw_steps
    }

    pub fn input_mode(&self) -> InputMode {
        self.input_mode
    }

    /// タップ位置を表示中の slot と Card の行に照合する。y は画面上端からの位置。
    /// Card の行は画面幅いっぱいに置かれるため、横方向の位置は照合に使わない。
    pub fn hit(&self, y: u16) -> Event {
        let (slot, top) = if self.state.get(Slot::Overlay).is_some() {
            (Slot::Overlay, 0)
        } else if y < BANNER_HEIGHT && self.state.get(Slot::BannerTop).is_some() {
            (Slot::BannerTop, 0)
        } else if (BOTTOM_BANNER_Y..BOTTOM_BANNER_Y + BANNER_HEIGHT).contains(&y)
            && self.state.get(Slot::BannerBottom).is_some()
        {
            (Slot::BannerBottom, BOTTOM_BANNER_Y)
        } else {
            return Event::Tap {
                slot: None,
                card: None,
                action: None,
            };
        };
        let Some(Entry {
            content: Content::Card(card),
            ..
        }) = self.state.get(slot)
        else {
            return Event::Tap {
                slot: Some(slot),
                card: None,
                action: None,
            };
        };
        let mut row_top = top + CARD_TOP_MARGIN;
        let mut action = None;
        for row in &card.rows {
            let height = row.height();
            if (row_top..row_top + height).contains(&y) {
                action = row.action;
                break;
            }
            row_top += height;
        }
        Event::Tap {
            slot: Some(slot),
            card: (card.id != 0).then_some(card.id),
            action,
        }
    }

    pub fn actuator_target(&self) -> crate::behavior::ActuatorTarget {
        crate::behavior::target(self.state.presence(), self.state.emote())
    }

    pub fn handle<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        message: Message,
        now_ms: u64,
        display: &mut D,
    ) -> Result<Reply, HandleError<D::Error>> {
        let mut next = self.state.clone();
        match message {
            Message::Ping { nonce } => {
                return Ok(Reply::Pong {
                    nonce,
                    version: protocol::VERSION,
                });
            }
            Message::Clear => next = DisplayState::default(),
            Message::ClearSlot(slot) => next.slots[slot_index(slot)] = None,
            Message::InputMode(mode) => {
                // 表示を変えないため描画せず、PitchTrim と同様に通し番号だけを進める。
                self.input_mode = mode;
                self.seq = self.seq.wrapping_add(1);
                return Ok(Reply::Ack { seq: self.seq });
            }
            Message::Text { slot, ttl_s, text } => {
                if self.demo_index.is_some() {
                    next.presence = None;
                }
                next.set(slot, Content::Text(text), ttl_s, now_ms)
            }
            Message::Card(card) => {
                card.validate().map_err(|_| HandleError::InvalidCard)?;
                if self.demo_index.is_some() {
                    next.presence = None;
                }
                let slot = card.slot;
                let ttl_s = card.ttl_s;
                next.set(slot, Content::Card(card), ttl_s, now_ms);
            }
            Message::Presence(presence) => next.set_presence(presence, now_ms),
            Message::Emote(emote) => {
                emote.validate().map_err(|_| HandleError::InvalidFace)?;
                next.set_emote(emote, now_ms);
            }
            Message::PitchTrim(trim) => {
                trim.validate().map_err(|_| HandleError::InvalidFace)?;
                self.pitch_trim_raw_steps = trim.raw_steps;
                self.seq = self.seq.wrapping_add(1);
                return Ok(Reply::Ack { seq: self.seq });
            }
            // 実機の I²C bus は main が所有する。誤ってモデルへ渡されても描画しない。
            Message::HardwareProbe => return Err(HandleError::InvalidFace),
            // 開始と行の受信は描画しない。画素は ImageEnd で表示する。
            Message::ImageBegin(begin) => {
                begin.validate().map_err(|_| HandleError::InvalidImage)?;
                self.pending_image = Some(PendingImage { begin, next_row: 0 });
                self.seq = self.seq.wrapping_add(1);
                return Ok(Reply::Ack { seq: self.seq });
            }
            Message::ImageRows(rows) => {
                let pending = self
                    .pending_image
                    .as_mut()
                    .filter(|pending| pending.begin.id == rows.id)
                    .ok_or(HandleError::InvalidImage)?;
                let row_bytes = usize::from(pending.begin.width) * 2;
                let count = rows.pixels.len() / row_bytes;
                if rows.pixels.is_empty()
                    || rows.pixels.len() % row_bytes != 0
                    || rows.row != pending.next_row
                    || usize::from(rows.row) + count > usize::from(pending.begin.height)
                {
                    return Err(HandleError::InvalidImage);
                }
                let start = usize::from(rows.row) * row_bytes;
                self.image.pixels[start..start + rows.pixels.len()].copy_from_slice(&rows.pixels);
                pending.next_row += count as u16;
                self.seq = self.seq.wrapping_add(1);
                return Ok(Reply::Ack { seq: self.seq });
            }
            Message::ImageEnd { id } => {
                let pending = self.pending_image.take().ok_or(HandleError::InvalidImage)?;
                if pending.begin.id != id || pending.next_row != pending.begin.height {
                    return Err(HandleError::InvalidImage);
                }
                let begin = pending.begin;
                let region = ImageRegion {
                    x: begin.x,
                    y: begin.y,
                    width: begin.width,
                    height: begin.height,
                };
                next.set(Slot::Overlay, Content::Image(region), begin.ttl_s, now_ms);
            }
        }
        next.presence()
            .map(|presence| presence.gaze.validate())
            .transpose()
            .map_err(|_| HandleError::InvalidFace)?;
        // Overlay の背後の期限切れも、再表示の前に除去する。
        next.expire(now_ms);
        let blink = blink_at(now_ms);
        crate::renderer::draw_state_with_image(display, &next, &self.image.pixels, blink)
            .map_err(HandleError::Draw)?;
        self.state = next;
        self.blink = blink;
        self.startup = false;
        self.seq = self.seq.wrapping_add(1);
        self.demo_index = None;
        Ok(Reply::Ack { seq: self.seq })
    }

    pub fn tap<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        now_ms: u64,
        display: &mut D,
    ) -> Result<(), D::Error> {
        let index = self
            .demo_index
            .map_or(0, |previous| (previous + 1) % DEMO_FACE_COUNT);
        let (expression, label) = DEMO_FACES[index];
        let mut next = self.state.clone();
        // 全画面 Overlay 中でも、タップした表情をその場で見られるようにする。
        next.slots[slot_index(Slot::Overlay)] = None;
        next.emote = None;
        next.set_presence(
            Presence {
                activity: None,
                detail: label.try_into().expect("demo label fits"),
                expression,
                gaze: Gaze::Center,
                eyes: EyeStyle::Auto,
                ttl_s: 0,
            },
            now_ms,
        );
        next.expire(now_ms);
        let blink = blink_at(now_ms);
        crate::renderer::draw_state_with_image(display, &next, &self.image.pixels, blink)?;
        self.state = next;
        self.blink = blink;
        self.startup = false;
        self.demo_index = Some(index);
        Ok(())
    }

    pub fn tick<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        now_ms: u64,
        display: &mut D,
    ) -> Result<(), D::Error> {
        let blink = blink_at(now_ms);
        if self.state.has_expired(now_ms) || (self.state.can_blink() && blink != self.blink) {
            let mut next = self.state.clone();
            next.expire(now_ms);
            if self.startup {
                crate::renderer::draw_startup_with_blink(display, blink)?;
            } else {
                crate::renderer::draw_state_with_image(display, &next, &self.image.pixels, blink)?;
            }
            self.state = next;
            self.blink = blink;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics::prelude::*;

    #[derive(Default)]
    struct Display {
        fail: bool,
        pixels: usize,
    }
    impl OriginDimensions for Display {
        fn size(&self) -> Size {
            Size::new(320, 240)
        }
    }
    impl DrawTarget for Display {
        type Color = Rgb565;
        type Error = ();
        fn draw_iter<I: IntoIterator<Item = Pixel<Rgb565>>>(
            &mut self,
            pixels: I,
        ) -> Result<(), ()> {
            if self.fail {
                return Err(());
            }
            self.pixels += pixels.into_iter().count();
            Ok(())
        }
    }

    fn text(slot: Slot, ttl_s: u16, content: &str) -> Message {
        Message::Text {
            slot,
            ttl_s,
            text: content.try_into().unwrap(),
        }
    }

    fn card(slot: Slot, ttl_s: u16, ratio: u8) -> Message {
        let mut rows = heapless::Vec::new();
        rows.push(protocol::Row {
            elements: heapless::Vec::from_slice(&[protocol::Element::Text {
                text: "CPU".try_into().unwrap(),
            }])
            .unwrap(),
            action: None,
        })
        .unwrap();
        rows.push(protocol::Row {
            elements: heapless::Vec::from_slice(&[protocol::Element::Bar {
                ratio,
                label: "Usage".try_into().unwrap(),
            }])
            .unwrap(),
            action: None,
        })
        .unwrap();
        Message::Card(Card {
            slot,
            ttl_s,
            id: 0,
            rows,
            image: None,
        })
    }

    #[test]
    fn pitch_trim_is_volatile_and_does_not_redraw_or_clear_with_face() {
        let mut controller = Controller::default();
        let mut display = Display::default();
        assert_eq!(controller.pitch_trim_raw_steps(), 0);
        assert_eq!(
            controller.handle(
                Message::PitchTrim(protocol::PitchTrim { raw_steps: -24 }),
                0,
                &mut display,
            ),
            Ok(Reply::Ack { seq: 1 })
        );
        assert_eq!(display.pixels, 0);
        assert_eq!(controller.pitch_trim_raw_steps(), -24);
        controller.handle(Message::Clear, 1, &mut display).unwrap();
        assert_eq!(controller.pitch_trim_raw_steps(), -24);
        assert_eq!(
            controller.handle(
                Message::PitchTrim(protocol::PitchTrim { raw_steps: -97 }),
                2,
                &mut display,
            ),
            Err(HandleError::InvalidFace)
        );
        assert_eq!(controller.pitch_trim_raw_steps(), -24);
    }

    #[test]
    fn card_replaces_slot_and_expires_without_affecting_other_slots() {
        let mut controller = Controller::default();
        let mut display = Display::default();
        controller
            .handle(text(Slot::BannerBottom, 0, "keep"), 0, &mut display)
            .unwrap();
        assert_eq!(
            controller.handle(card(Slot::BannerTop, 1, 75), 100, &mut display),
            Ok(Reply::Ack { seq: 2 })
        );
        assert!(matches!(
            controller.state.get(Slot::BannerTop).unwrap().content,
            Content::Card(_)
        ));
        controller.tick(1100, &mut display).unwrap();
        assert!(controller.state.get(Slot::BannerTop).is_none());
        assert!(controller.state.get(Slot::BannerBottom).is_some());
    }

    #[test]
    fn invalid_card_is_rejected_before_drawing_or_state_change() {
        let mut controller = Controller::default();
        let mut display = Display::default();
        let pixels = display.pixels;
        assert_eq!(
            controller.handle(card(Slot::BannerTop, 0, 101), 0, &mut display),
            Err(HandleError::InvalidCard)
        );
        assert_eq!(display.pixels, pixels);
        assert_eq!(controller.seq, 0);
        assert_eq!(controller.state, DisplayState::default());
    }

    #[test]
    fn presence_expires_independently_of_slots_and_overlay() {
        let mut controller = Controller::default();
        let mut display = Display::default();
        controller
            .handle(text(Slot::BannerBottom, 0, "keep"), 0, &mut display)
            .unwrap();
        let presence = Presence {
            activity: Some(protocol::Activity::Working),
            detail: "Build".try_into().unwrap(),
            expression: protocol::Expression::Focused,
            gaze: protocol::Gaze::Right,
            eyes: protocol::EyeStyle::Auto,
            ttl_s: 1,
        };
        assert_eq!(
            controller.handle(Message::Presence(presence.clone()), 100, &mut display),
            Ok(Reply::Ack { seq: 2 })
        );
        assert_eq!(controller.state.presence(), Some(&presence));
        controller
            .handle(text(Slot::Overlay, 0, "overlay"), 200, &mut display)
            .unwrap();
        controller.tick(1099, &mut display).unwrap();
        assert!(controller.state.presence().is_some());
        controller.tick(1100, &mut display).unwrap();
        assert!(controller.state.presence().is_none());
        assert!(controller.state.get(Slot::Overlay).is_some());
        assert!(controller.state.get(Slot::BannerBottom).is_some());
        assert_eq!(controller.seq, 3);
    }

    #[test]
    fn emote_expires_to_previous_presence_and_invalid_values_are_rejected() {
        let mut controller = Controller::default();
        let mut display = Display::default();
        let presence = Presence {
            activity: None,
            detail: Default::default(),
            expression: Expression::Calm,
            gaze: Gaze::Center,
            eyes: EyeStyle::Auto,
            ttl_s: 0,
        };
        controller
            .handle(Message::Presence(presence.clone()), 0, &mut display)
            .unwrap();
        let emote = Emote {
            expression: Expression::Curious,
            gaze: Gaze::Point { x: 50, y: -50 },
            eyes: EyeStyle::Wide,
            intensity: 50,
            duration_ms: 500,
        };
        assert_eq!(
            controller.handle(Message::Emote(emote), 100, &mut display),
            Ok(Reply::Ack { seq: 2 })
        );
        assert_eq!(controller.state.emote(), Some(&emote));
        assert_eq!(controller.state.presence(), Some(&presence));
        assert_eq!(controller.actuator_target().x_tenth_deg, 75);
        controller.tick(599, &mut display).unwrap();
        assert_eq!(controller.state.emote(), Some(&emote));
        controller.tick(600, &mut display).unwrap();
        assert_eq!(controller.state.emote(), None);
        assert_eq!(controller.state.presence(), Some(&presence));
        assert_eq!(controller.actuator_target().x_tenth_deg, 0);
        assert_eq!(controller.seq, 2);

        let pixels = display.pixels;
        assert_eq!(
            controller.handle(
                Message::Emote(Emote {
                    duration_ms: 0,
                    ..emote
                }),
                700,
                &mut display,
            ),
            Err(HandleError::InvalidFace)
        );
        assert_eq!(display.pixels, pixels);
        assert_eq!(controller.state.presence(), Some(&presence));
    }

    #[test]
    fn taps_show_every_expression_and_wrap_without_usb_sequence() {
        let mut controller = Controller::default();
        let mut display = Display::default();
        controller
            .handle(text(Slot::BannerBottom, 0, "keep"), 0, &mut display)
            .unwrap();
        controller
            .handle(text(Slot::Overlay, 0, "overlay"), 0, &mut display)
            .unwrap();
        let seq = controller.seq;
        for (index, (expression, label)) in DEMO_FACES.iter().enumerate() {
            controller.tap(index as u64, &mut display).unwrap();
            let presence = controller.state.presence().unwrap();
            assert_eq!(presence.expression, *expression);
            assert_eq!(presence.detail.as_str(), *label);
            assert_eq!(controller.demo_index, Some(index));
            assert!(controller.state.get(Slot::Overlay).is_none());
            assert!(controller.state.get(Slot::BannerBottom).is_some());
            assert_eq!(controller.seq, seq);
        }
        controller.tap(12, &mut display).unwrap();
        assert_eq!(
            controller.state.presence().unwrap().expression,
            Expression::Happy
        );
        controller
            .handle(text(Slot::BannerTop, 0, "host"), 13, &mut display)
            .unwrap();
        assert!(controller.state.presence().is_none());
        controller.tap(14, &mut display).unwrap();
        assert_eq!(
            controller.state.presence().unwrap().detail.as_str(),
            "01/12 HAPPY"
        );
    }

    #[test]
    fn failed_tap_does_not_advance_demo() {
        let mut controller = Controller::default();
        let mut display = Display {
            fail: true,
            pixels: 0,
        };
        assert_eq!(controller.tap(0, &mut display), Err(()));
        assert_eq!(controller.demo_index, None);
        assert_eq!(controller.state, DisplayState::default());
        assert_eq!(controller.seq, 0);
    }

    #[test]
    fn slots_expire_independently_at_deadline_and_zero_persists() {
        let mut controller = Controller::default();
        let mut display = Display::default();
        for (slot, ttl) in [
            (Slot::BannerTop, 1),
            (Slot::BannerBottom, 0),
            (Slot::Overlay, 2),
        ] {
            controller
                .handle(text(slot, ttl, "test"), 100, &mut display)
                .unwrap();
        }
        let pixels = display.pixels;
        controller.tick(1099, &mut display).unwrap();
        assert_eq!(display.pixels, pixels);
        controller.tick(1100, &mut display).unwrap();
        assert!(controller.state.get(Slot::BannerTop).is_none());
        assert!(controller.state.get(Slot::Overlay).is_some());
        controller.tick(2100, &mut display).unwrap();
        assert!(controller.state.get(Slot::Overlay).is_none());
        assert!(controller.state.get(Slot::BannerBottom).is_some());
    }

    #[test]
    fn replacing_slot_restarts_ttl_and_clear_removes_every_slot() {
        let mut controller = Controller::default();
        let mut display = Display::default();
        controller
            .handle(text(Slot::BannerTop, 1, "old"), 0, &mut display)
            .unwrap();
        controller
            .handle(text(Slot::BannerTop, 2, "new"), 500, &mut display)
            .unwrap();
        controller.tick(1000, &mut display).unwrap();
        assert_eq!(
            controller.state.get(Slot::BannerTop).unwrap().content,
            Content::Text("new".try_into().unwrap())
        );
        assert_eq!(
            controller.handle(Message::Clear, 1000, &mut display),
            Ok(Reply::Ack { seq: 3 })
        );
        assert_eq!(controller.state, DisplayState::default());
        let pixels = display.pixels;
        controller.tick(5000, &mut display).unwrap();
        assert_eq!(display.pixels, pixels);
    }

    #[test]
    fn failed_drawing_does_not_commit_state_or_sequence_and_expiry_retries() {
        let mut controller = Controller::default();
        let mut display = Display::default();
        controller
            .handle(text(Slot::BannerTop, 1, "keep"), 0, &mut display)
            .unwrap();
        let before = controller.state.clone();
        display.fail = true;
        assert_eq!(
            controller.handle(Message::Clear, 1, &mut display),
            Err(HandleError::Draw(()))
        );
        assert_eq!(controller.state, before);
        assert_eq!(controller.seq, 1);
        assert_eq!(controller.tick(1000, &mut display), Err(()));
        assert_eq!(controller.state, before);
        display.fail = false;
        controller.tick(1000, &mut display).unwrap();
        assert_eq!(controller.state, DisplayState::default());
        assert_eq!(controller.seq, 1);
    }

    #[test]
    fn ping_does_not_draw_and_ack_sequence_wraps() {
        let mut controller = Controller {
            seq: u32::MAX,
            ..Controller::default()
        };
        let mut display = Display::default();
        assert_eq!(
            controller.handle(Message::Ping { nonce: 42 }, 0, &mut display),
            Ok(Reply::Pong {
                nonce: 42,
                version: protocol::VERSION
            })
        );
        assert_eq!(display.pixels, 0);
        assert_eq!(
            controller.handle(Message::Clear, 0, &mut display),
            Ok(Reply::Ack { seq: 0 })
        );
    }

    fn card_with_actions(slot: Slot, id: u16) -> Message {
        let Message::Card(mut card) = card(slot, 0, 50) else {
            unreachable!()
        };
        card.id = id;
        card.rows[0].action = Some(7);
        Message::Card(card)
    }

    #[test]
    fn taps_resolve_to_slot_card_and_row_action() {
        let mut controller = Controller::default();
        let mut display = Display::default();
        let none = Event::Tap {
            slot: None,
            card: None,
            action: None,
        };
        assert_eq!(controller.hit(10), none);
        controller
            .handle(card_with_actions(Slot::BannerTop, 42), 0, &mut display)
            .unwrap();
        controller
            .handle(text(Slot::BannerBottom, 0, "note"), 0, &mut display)
            .unwrap();
        // 1 行目は y 4..24 (action 7)、2 行目は 24..44 (action なし)。
        for (y, action) in [
            (4, Some(7)),
            (23, Some(7)),
            (24, None),
            (47, None),
            (0, None),
        ] {
            assert_eq!(
                controller.hit(y),
                Event::Tap {
                    slot: Some(Slot::BannerTop),
                    card: Some(42),
                    action
                },
                "y={y}"
            );
        }
        assert_eq!(controller.hit(120), none, "顔の領域");
        assert_eq!(
            controller.hit(200),
            Event::Tap {
                slot: Some(Slot::BannerBottom),
                card: None,
                action: None
            }
        );
        // Overlay は画面全体を覆い、識別子 0 の Card は card を返さない。
        controller
            .handle(card_with_actions(Slot::Overlay, 0), 0, &mut display)
            .unwrap();
        assert_eq!(
            controller.hit(10),
            Event::Tap {
                slot: Some(Slot::Overlay),
                card: None,
                action: Some(7)
            }
        );
    }

    #[test]
    fn clear_slot_removes_only_that_slot() {
        let mut controller = Controller::default();
        let mut display = Display::default();
        controller
            .handle(text(Slot::BannerTop, 0, "top"), 0, &mut display)
            .unwrap();
        controller
            .handle(text(Slot::BannerBottom, 0, "bottom"), 0, &mut display)
            .unwrap();
        assert_eq!(
            controller.handle(Message::ClearSlot(Slot::BannerTop), 0, &mut display),
            Ok(Reply::Ack { seq: 3 })
        );
        assert!(controller.state.get(Slot::BannerTop).is_none());
        assert!(controller.state.get(Slot::BannerBottom).is_some());
    }

    #[test]
    fn input_mode_is_acknowledged_without_redraw() {
        let mut controller = Controller::default();
        let mut display = Display::default();
        assert_eq!(controller.input_mode(), InputMode::Demo);
        assert_eq!(
            controller.handle(Message::InputMode(InputMode::Forward), 0, &mut display),
            Ok(Reply::Ack { seq: 1 })
        );
        assert_eq!(controller.input_mode(), InputMode::Forward);
        assert_eq!(display.pixels, 0);
        assert!(controller.startup, "表示確認画面を維持する");
    }

    fn image_rows(id: u16, row: u16, rows: usize, width: usize, value: u8) -> Message {
        Message::ImageRows(protocol::ImageRows {
            id,
            row,
            pixels: heapless::Vec::from_slice(&std::vec![value; rows * width * 2]).unwrap(),
        })
    }

    fn begin(id: u16, width: u16, height: u16) -> Message {
        Message::ImageBegin(ImageBegin {
            id,
            x: 10,
            y: 20,
            width,
            height,
            ttl_s: 5,
        })
    }

    #[test]
    fn image_is_shown_only_after_all_rows_arrive() {
        let mut controller = Controller::default();
        let mut frame = std::boxed::Box::new(crate::framebuffer::FrameBuffer::new());
        assert_eq!(
            controller.handle(begin(7, 4, 3), 0, &mut *frame),
            Ok(Reply::Ack { seq: 1 })
        );
        // 1 行目は赤 (0xF800)、残り 2 行は青 (0x001F)。
        let red = Message::ImageRows(protocol::ImageRows {
            id: 7,
            row: 0,
            pixels: heapless::Vec::from_slice(&[0xF8, 0x00].repeat(4)).unwrap(),
        });
        let blue = Message::ImageRows(protocol::ImageRows {
            id: 7,
            row: 1,
            pixels: heapless::Vec::from_slice(&[0x00, 0x1F].repeat(8)).unwrap(),
        });
        assert_eq!(
            controller.handle(red, 0, &mut *frame),
            Ok(Reply::Ack { seq: 2 })
        );
        assert_eq!(
            controller.handle(blue, 0, &mut *frame),
            Ok(Reply::Ack { seq: 3 })
        );
        assert!(
            controller.state.get(Slot::Overlay).is_none(),
            "End までは表示しない"
        );
        assert_eq!(
            controller.handle(Message::ImageEnd { id: 7 }, 0, &mut *frame),
            Ok(Reply::Ack { seq: 4 })
        );
        use embedded_graphics::pixelcolor::RgbColor;
        assert_eq!(
            frame.pixel(10, 20),
            embedded_graphics::pixelcolor::Rgb565::RED
        );
        assert_eq!(
            frame.pixel(13, 22),
            embedded_graphics::pixelcolor::Rgb565::BLUE
        );
        assert_eq!(
            frame.pixel(14, 20),
            embedded_graphics::pixelcolor::Rgb565::BLACK
        );
        assert_eq!(
            controller.hit(21),
            Event::Tap {
                slot: Some(Slot::Overlay),
                card: None,
                action: None
            }
        );
    }

    #[test]
    fn malformed_image_sequences_are_rejected() {
        let mut controller = Controller::default();
        let mut display = Display::default();
        let invalid = |result: Result<Reply, HandleError<()>>| {
            assert_eq!(result, Err(HandleError::InvalidImage));
        };
        invalid(controller.handle(image_rows(1, 0, 1, 4, 0), 0, &mut display));
        invalid(controller.handle(Message::ImageEnd { id: 1 }, 0, &mut display));
        invalid(controller.handle(begin(1, 161, 1), 0, &mut display));

        controller.handle(begin(1, 4, 2), 0, &mut display).unwrap();
        invalid(controller.handle(image_rows(2, 0, 1, 4, 0), 0, &mut display));
        invalid(controller.handle(image_rows(1, 1, 1, 4, 0), 0, &mut display));
        invalid(controller.handle(image_rows(1, 0, 3, 4, 0), 0, &mut display));
        controller
            .handle(image_rows(1, 0, 1, 4, 0), 0, &mut display)
            .unwrap();
        // 行が足りない End は拒否し、受信中の画像を破棄する。
        invalid(controller.handle(Message::ImageEnd { id: 1 }, 0, &mut display));
        invalid(controller.handle(image_rows(1, 1, 1, 4, 0), 0, &mut display));
        assert_eq!(display.pixels, 0, "開始と行の受信は描画しない");
    }
}
