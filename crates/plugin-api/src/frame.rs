//! stdio 上のフレームの読み書き。
//!
//! 受信側は `MAX_FRAME_BYTES` を超えるフレームを保持せずに次の 0x00 まで読み捨てる。
//! 相手が不正な plugin であっても、daemon の確保量がフレーム長の上限で抑えられる。

use std::io::{self, BufRead, BufReader, Read, Write};

use serde::{Serialize, de::DeserializeOwned};

use crate::MAX_FRAME_BYTES;

#[derive(Debug)]
pub enum FrameError {
    Io(io::Error),
    /// `MAX_FRAME_BYTES` を超えるフレーム。受信側では次の区切りまで読み捨てた。
    Oversized,
    /// COBS または postcard として復号できないフレーム。
    Malformed,
    /// 区切りの前に入力が終わった。
    Truncated,
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(f, "I/O: {error}"),
            Self::Oversized => f.write_str("フレームが上限を超えています"),
            Self::Malformed => f.write_str("フレームを復号できません"),
            Self::Truncated => f.write_str("フレームの途中で入力が終わりました"),
        }
    }
}

impl std::error::Error for FrameError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for FrameError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// 値を 1 フレームとして書き、flush する。上限を超える値は書かずに失敗する。
pub fn write_frame<T: Serialize>(writer: &mut impl Write, value: &T) -> Result<(), FrameError> {
    let frame = postcard::to_stdvec_cobs(value).map_err(|_| FrameError::Malformed)?;
    if frame.len() > MAX_FRAME_BYTES {
        return Err(FrameError::Oversized);
    }
    writer.write_all(&frame)?;
    writer.flush()?;
    Ok(())
}

/// 0x00 区切りのフレームを順に復号する。
pub struct FrameReader<R> {
    inner: BufReader<R>,
    frame: Vec<u8>,
    discarding: bool,
    finished: bool,
}

impl<R: Read> FrameReader<R> {
    pub fn new(inner: R) -> Self {
        Self {
            inner: BufReader::new(inner),
            frame: Vec::new(),
            discarding: false,
            finished: false,
        }
    }

    /// 次のフレームを復号する。入力の終端では `Ok(None)` を返す。
    ///
    /// 不正なフレームはエラーとして返すが、読取り位置は次のフレームの先頭に進んでいる。
    /// 呼出し側は続行するか、相手を切断するかを選べる。
    pub fn read_frame<T: DeserializeOwned>(&mut self) -> Result<Option<T>, FrameError> {
        loop {
            if self.finished {
                return Ok(None);
            }
            let available = match self.inner.fill_buf() {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            };
            if available.is_empty() {
                self.finished = true;
                if self.frame.is_empty() && !self.discarding {
                    return Ok(None);
                }
                self.frame.clear();
                self.discarding = false;
                return Err(FrameError::Truncated);
            }
            let (chunk, terminated) = match available.iter().position(|&byte| byte == 0) {
                Some(index) => (&available[..index], true),
                None => (available, false),
            };
            let consumed = chunk.len() + usize::from(terminated);
            if !self.discarding {
                // 区切りの 0x00 を含めた長さで上限を判定する (送信側の判定と揃える)。
                if self.frame.len() + chunk.len() + 1 > MAX_FRAME_BYTES {
                    self.frame.clear();
                    self.discarding = true;
                } else {
                    self.frame.extend_from_slice(chunk);
                }
            }
            self.inner.consume(consumed);
            if !terminated {
                continue;
            }
            if self.discarding {
                self.discarding = false;
                return Err(FrameError::Oversized);
            }
            if self.frame.is_empty() {
                // 空の区切りは同期用として読み飛ばす。
                continue;
            }
            let result = postcard::from_bytes_cobs(&mut self.frame);
            self.frame.clear();
            return result.map(Some).map_err(|_| FrameError::Malformed);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HostMessage, LogLevel, PluginMessage};

    fn encoded(message: &PluginMessage) -> Vec<u8> {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, message).unwrap();
        bytes
    }

    fn log(text: &str) -> PluginMessage {
        PluginMessage::Log {
            level: LogLevel::Info,
            text: text.into(),
        }
    }

    #[test]
    fn frames_roundtrip_across_arbitrary_read_boundaries() {
        let mut bytes = vec![0, 0];
        bytes.extend(encoded(&log("first")));
        bytes.push(0);
        bytes.extend(encoded(&PluginMessage::CardRemove { card: 3 }));
        // 1 byte ずつしか読めない入力でも同じ結果になる。
        struct OneByte<'a>(&'a [u8]);
        impl Read for OneByte<'_> {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                let Some((&first, rest)) = self.0.split_first() else {
                    return Ok(0);
                };
                buf[0] = first;
                self.0 = rest;
                Ok(1)
            }
        }
        let mut reader = FrameReader::new(OneByte(&bytes));
        assert_eq!(
            reader.read_frame::<PluginMessage>().unwrap(),
            Some(log("first"))
        );
        assert_eq!(
            reader.read_frame::<PluginMessage>().unwrap(),
            Some(PluginMessage::CardRemove { card: 3 })
        );
        assert!(reader.read_frame::<PluginMessage>().unwrap().is_none());
    }

    #[test]
    fn oversized_frame_is_discarded_and_reader_resynchronizes() {
        let mut bytes = vec![0xFF; MAX_FRAME_BYTES + 10];
        bytes.push(0);
        bytes.extend(encoded(&log("after")));
        let mut reader = FrameReader::new(bytes.as_slice());
        assert!(matches!(
            reader.read_frame::<PluginMessage>(),
            Err(FrameError::Oversized)
        ));
        assert_eq!(
            reader.read_frame::<PluginMessage>().unwrap(),
            Some(log("after"))
        );
    }

    #[test]
    fn frame_at_limit_is_accepted() {
        // COBS の付加バイトと区切りを含めて上限に収まる最長の Log を二分探索で求める。
        let wire_len = |len: usize| {
            postcard::to_stdvec_cobs(&log(&"x".repeat(len)))
                .unwrap()
                .len()
        };
        let (mut fits, mut exceeds) = (0, MAX_FRAME_BYTES);
        while exceeds - fits > 1 {
            let middle = (fits + exceeds) / 2;
            if wire_len(middle) <= MAX_FRAME_BYTES {
                fits = middle;
            } else {
                exceeds = middle;
            }
        }
        let bytes = encoded(&log(&"x".repeat(fits)));
        assert!(MAX_FRAME_BYTES - bytes.len() < 4);
        let mut reader = FrameReader::new(bytes.as_slice());
        assert!(reader.read_frame::<PluginMessage>().unwrap().is_some());
        let mut sink = Vec::new();
        assert!(matches!(
            write_frame(&mut sink, &log(&"x".repeat(exceeds))),
            Err(FrameError::Oversized)
        ));
        assert!(sink.is_empty());
    }

    #[test]
    fn malformed_and_truncated_frames_are_reported() {
        let mut bytes = vec![0x01, 0xFF, 0xFF, 0];
        bytes.extend(&encoded(&log("ok"))[..3]);
        let mut reader = FrameReader::new(bytes.as_slice());
        assert!(matches!(
            reader.read_frame::<HostMessage>(),
            Err(FrameError::Malformed)
        ));
        assert!(matches!(
            reader.read_frame::<PluginMessage>(),
            Err(FrameError::Truncated)
        ));
        assert!(reader.read_frame::<PluginMessage>().unwrap().is_none());
    }
}
