//! Slot の内容と単調時計に基づく表示期限。描画の成功後に状態を確定する。

use embedded_graphics::{pixelcolor::Rgb565, prelude::DrawTarget};
use protocol::{MAX_TEXT_BYTES, Message, Reply, Slot};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub text: heapless::String<MAX_TEXT_BYTES>,
    deadline_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisplayState {
    slots: [Option<Entry>; 3],
}

impl DisplayState {
    pub fn get(&self, slot: Slot) -> Option<&Entry> {
        self.slots[slot_index(slot)].as_ref()
    }

    fn set(&mut self, slot: Slot, text: heapless::String<MAX_TEXT_BYTES>, ttl_s: u16, now_ms: u64) {
        self.slots[slot_index(slot)] = Some(Entry {
            text,
            deadline_ms: (ttl_s != 0).then(|| now_ms.saturating_add(u64::from(ttl_s) * 1000)),
        });
    }

    fn has_expired(&self, now_ms: u64) -> bool {
        self.slots
            .iter()
            .flatten()
            .any(|entry| entry.deadline_ms.is_some_and(|deadline| now_ms >= deadline))
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
    }
}

fn slot_index(slot: Slot) -> usize {
    match slot {
        Slot::BannerTop => 0,
        Slot::BannerBottom => 1,
        Slot::Overlay => 2,
    }
}

#[derive(Default)]
pub struct Controller {
    state: DisplayState,
    seq: u32,
}

impl Controller {
    pub fn handle<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        message: Message,
        now_ms: u64,
        display: &mut D,
    ) -> Result<Reply, D::Error> {
        let mut next = self.state.clone();
        match message {
            Message::Ping { nonce } => {
                return Ok(Reply::Pong {
                    nonce,
                    version: protocol::VERSION,
                });
            }
            Message::Clear => next = DisplayState::default(),
            Message::Text { slot, ttl_s, text } => next.set(slot, text, ttl_s, now_ms),
        }
        // Overlay の背後の期限切れも、再表示の前に除去する。
        next.expire(now_ms);
        crate::renderer::draw_state(display, &next)?;
        self.state = next;
        self.seq = self.seq.wrapping_add(1);
        Ok(Reply::Ack { seq: self.seq })
    }

    pub fn tick<D: DrawTarget<Color = Rgb565>>(
        &mut self,
        now_ms: u64,
        display: &mut D,
    ) -> Result<(), D::Error> {
        if self.state.has_expired(now_ms) {
            let mut next = self.state.clone();
            next.expire(now_ms);
            crate::renderer::draw_state(display, &next)?;
            self.state = next;
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
        assert_eq!(controller.state.get(Slot::BannerTop).unwrap().text, "new");
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
        assert_eq!(controller.handle(Message::Clear, 1, &mut display), Err(()));
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
}
