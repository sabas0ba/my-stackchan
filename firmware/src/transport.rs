//! USB のパケット境界に依存しない、固定長のフレーム受信処理。

use protocol::{MAX_FRAME_BYTES, Message, Reply};

pub struct Receiver {
    frame: [u8; MAX_FRAME_BYTES],
    len: usize,
    discarding: bool,
    rejected: u32,
}

impl Default for Receiver {
    fn default() -> Self {
        Self {
            frame: [0; MAX_FRAME_BYTES],
            len: 0,
            discarding: false,
            rejected: 0,
        }
    }
}

impl Receiver {
    /// 完全なフレームを受け取ったときだけメッセージまたは拒否応答を返す。
    pub fn push(&mut self, byte: u8) -> Option<Result<Message, Reply>> {
        if self.discarding {
            if byte == 0 {
                self.discarding = false;
                return Some(Err(self.reject()));
            }
            return None;
        }

        if byte != 0 {
            // 終端の 1 byte を含めて MAX_FRAME_BYTES に収める。
            if self.len == MAX_FRAME_BYTES - 1 {
                self.len = 0;
                self.discarding = true;
            } else {
                self.frame[self.len] = byte;
                self.len += 1;
            }
            return None;
        }

        // 接続時の同期用区切りは、空のメッセージとして数えない。
        if self.len == 0 {
            return None;
        }
        self.frame[self.len] = 0;
        let message = protocol::decode::<Message>(&mut self.frame[..=self.len]);
        self.len = 0;
        Some(message.map_err(|_| self.reject()))
    }

    /// フレームの途中 (区切りの前) まで受け取っているか。
    pub fn in_frame(&self) -> bool {
        self.len != 0 || self.discarding
    }

    pub fn reject(&mut self) -> Reply {
        self.rejected = self.rejected.saturating_add(1);
        Reply::Rejected {
            count: self.rejected,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    fn frame(message: &Message) -> Vec<u8> {
        let mut buffer = [0; MAX_FRAME_BYTES];
        protocol::encode(message, &mut buffer).unwrap().to_vec()
    }

    fn receive(receiver: &mut Receiver, bytes: &[u8]) -> Vec<Result<Message, Reply>> {
        bytes
            .iter()
            .filter_map(|&byte| receiver.push(byte))
            .collect()
    }

    #[test]
    fn fragmented_ping_preserves_nonce_and_waits_for_delimiter() {
        let mut receiver = Receiver::default();
        for nonce in [0, 0x5A5A_1234, u32::MAX] {
            let frame = frame(&Message::Ping { nonce });
            assert!(receive(&mut receiver, &frame[..frame.len() - 1]).is_empty());
            assert_eq!(receiver.push(0), Some(Ok(Message::Ping { nonce })));
        }
    }

    #[test]
    fn in_frame_is_true_only_between_first_byte_and_delimiter() {
        let mut receiver = Receiver::default();
        let bytes = frame(&Message::Ping { nonce: 7 });
        assert!(!receiver.in_frame());
        receive(&mut receiver, &[0]);
        assert!(!receiver.in_frame(), "同期用の区切りだけでは途中としない");
        receive(&mut receiver, &bytes[..1]);
        assert!(receiver.in_frame());
        assert_eq!(receive(&mut receiver, &bytes[1..]).len(), 1);
        assert!(!receiver.in_frame());
        // 上限を超えて読み捨てている間も途中とする。
        receive(&mut receiver, &[1; MAX_FRAME_BYTES]);
        assert!(receiver.in_frame());
        receive(&mut receiver, &[0]);
        assert!(!receiver.in_frame());
    }

    #[test]
    fn consecutive_frames_and_empty_delimiters() {
        let mut receiver = Receiver::default();
        let mut stream = std::vec![0, 0];
        stream.extend(frame(&Message::Ping { nonce: 1 }));
        stream.extend(frame(&Message::Ping { nonce: 2 }));
        assert_eq!(
            receive(&mut receiver, &stream),
            [
                Ok(Message::Ping { nonce: 1 }),
                Ok(Message::Ping { nonce: 2 }),
            ]
        );
    }

    #[test]
    fn malformed_frame_does_not_prevent_display_messages_or_ping() {
        let mut receiver = Receiver::default();
        assert_eq!(
            receive(&mut receiver, &[0xFF, 1, 2, 0]),
            [Err(Reply::Rejected { count: 1 })]
        );
        assert_eq!(
            receive(&mut receiver, &frame(&Message::Clear)),
            [Ok(Message::Clear)]
        );
        let text = Message::Text {
            slot: protocol::Slot::Overlay,
            ttl_s: 1,
            text: heapless::String::try_from("test").unwrap(),
        };
        assert_eq!(receive(&mut receiver, &frame(&text)), [Ok(text)]);
        assert_eq!(
            receive(&mut receiver, &frame(&Message::Ping { nonce: 3 })),
            [Ok(Message::Ping { nonce: 3 })]
        );
    }

    #[test]
    fn oversized_frame_discards_valid_suffix_until_delimiter() {
        let mut receiver = Receiver::default();
        assert!(receive(&mut receiver, &[1; MAX_FRAME_BYTES * 2]).is_empty());
        let ping = frame(&Message::Ping { nonce: 4 });
        assert_eq!(
            receive(&mut receiver, &ping),
            [Err(Reply::Rejected { count: 1 })]
        );
        assert_eq!(
            receive(&mut receiver, &ping),
            [Ok(Message::Ping { nonce: 4 })]
        );
    }

    #[test]
    fn frame_at_capacity_rejects_once_and_recovers() {
        let mut receiver = Receiver::default();
        assert!(receive(&mut receiver, &[0xFF; MAX_FRAME_BYTES - 1]).is_empty());
        assert_eq!(receiver.push(0), Some(Err(Reply::Rejected { count: 1 })));
        assert_eq!(
            receive(&mut receiver, &frame(&Message::Ping { nonce: 5 })),
            [Ok(Message::Ping { nonce: 5 })]
        );
    }

    #[test]
    fn rejection_count_saturates() {
        let mut receiver = Receiver {
            rejected: u32::MAX,
            ..Receiver::default()
        };
        assert_eq!(
            receive(&mut receiver, &[0xFF, 0]),
            [Err(Reply::Rejected { count: u32::MAX })]
        );
    }

    #[test]
    fn arbitrary_streams_recover_at_next_delimiter() {
        let mut state = 0x6A1B_79C3_05D4_EF28u64;
        let ping = frame(&Message::Ping { nonce: 0x1234_5678 });
        for length in [
            0,
            1,
            254,
            255,
            MAX_FRAME_BYTES - 1,
            MAX_FRAME_BYTES,
            MAX_FRAME_BYTES + 1,
        ]
        .into_iter()
        .chain((0..256).map(|index| index * 7 % (MAX_FRAME_BYTES * 2)))
        {
            let mut receiver = Receiver::default();
            let mut input = std::vec![0; length];
            for byte in &mut input {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                *byte = state as u8;
            }
            let _ = receive(&mut receiver, &input);
            let _ = receiver.push(0);
            assert_eq!(
                receive(&mut receiver, &ping),
                [Ok(Message::Ping { nonce: 0x1234_5678 })]
            );
        }
    }
}
