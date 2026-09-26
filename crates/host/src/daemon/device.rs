//! daemon が保持するデバイスとの接続。
//!
//! 単発の CLI と異なり接続を保ち続けるため、次を扱う。
//! - 切断後の再接続と、接続時の版の照合と pitch trim の再適用
//! - Ping の nonce を送信ごとに変え、再接続前の古い応答と区別する
//! - Ack の通し番号の巻戻りから firmware の再起動を検出する
//! - 要求と独立に届く `Reply::Event` を、応答待ちの間も待機中も取りこぼさずに保持する

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use crate::PitchTrimFile;
use crate::log;

/// 切断後、次に接続を試みるまでの間隔。
const RECONNECT_INTERVAL: Duration = Duration::from_secs(2);
/// 応答を待つ最長時間。firmware は描画完了後に応答する。
const REPLY_TIMEOUT: Duration = Duration::from_secs(5);
/// 待機中の読取り 1 回の待ち時間。daemon のイベントループを止めないよう短くする。
const POLL_TIMEOUT: Duration = Duration::from_millis(1);
/// 要求の書込みの待ち時間。serialport は読取りと書込みで同じ待ち時間を用いるため、
/// 書込みの前に必ず設定し直す。待機中の読取りの値が残ると、1 USB パケットを超える
/// フレームの書込みが途中で打ち切られる (Windows で観測した)。
const WRITE_TIMEOUT: Duration = Duration::from_secs(1);
/// 応答の読取り 1 回の待ち時間。
const READ_TIMEOUT: Duration = Duration::from_millis(100);
/// 保持する Event の上限。daemon が取り出さない間に溢れた分は古いものから捨てる。
const MAX_PENDING_EVENTS: usize = 16;

/// 送信の結果。`Reset` はデバイスの表示状態が失われたことを表し、daemon は全 slot を送り直す。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    Delivered,
    Reset,
}

pub trait Device {
    fn send(&mut self, message: &protocol::Message, now: Instant) -> Result<Delivery, String>;
    /// 受信済みの Event を取り出す。接続していなければ何もしない。
    fn poll(&mut self) -> Vec<protocol::Event>;
}

/// USB から受け取ったもの。
#[derive(Debug, PartialEq, Eq)]
enum Received {
    Reply(protocol::Reply),
    /// フレームとして復号できないテキスト。firmware の起動ログや panic の出力であり、
    /// 調査のためにログへ残す。
    Text(String),
}

/// firmware のログとみなす条件。COBS のフレームは 0x01..0x1F の制御バイトをほぼ必ず含む
/// ため、改行・タブ・ESC (端末の色指定) 以外の制御文字を含まない UTF-8 だけをテキストとする。
fn as_text(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    let printable = text
        .chars()
        .all(|c| !c.is_control() || matches!(c, '\n' | '\r' | '\t' | '\x1b'));
    printable.then(|| text.to_owned())
}

/// 0x00 区切りのフレームを 1 byte ずつ組み立てる。上限を超えたフレームは次の区切りまで捨てる。
#[derive(Default)]
struct Frames {
    buffer: Vec<u8>,
    discarding: bool,
}

impl Frames {
    fn push(&mut self, byte: u8) -> Option<Received> {
        if byte != 0 {
            if self.discarding {
                return None;
            }
            if self.buffer.len() == protocol::MAX_FRAME_BYTES - 1 {
                // 長いテキストは区切りを待たずに出す。それ以外は次の区切りまで捨てる。
                if let Some(text) = as_text(&self.buffer) {
                    self.buffer.clear();
                    self.buffer.push(byte);
                    return Some(Received::Text(text));
                }
                self.buffer.clear();
                self.discarding = true;
                return None;
            }
            self.buffer.push(byte);
            return None;
        }
        let complete = !self.discarding && !self.buffer.is_empty();
        self.discarding = false;
        if !complete {
            self.buffer.clear();
            return None;
        }
        let text = as_text(&self.buffer);
        self.buffer.push(0);
        let received = match protocol::decode(&mut self.buffer) {
            Ok(reply) => Some(Received::Reply(reply)),
            Err(_) => text.map(Received::Text),
        };
        self.buffer.clear();
        received
    }

    /// 区切りが来ないまま残っているテキストを取り出す。firmware が panic で停止した場合、
    /// その出力の後に区切りは来ない。
    fn take_text(&mut self) -> Option<String> {
        if self.buffer.is_empty() {
            return None;
        }
        let text = as_text(&self.buffer);
        if text.is_some() {
            self.buffer.clear();
        }
        text
    }
}

/// firmware のテキスト出力を行ごとにログへ残す。panic と例外は error、それ以外は info とする。
fn log_device_text(text: &str) {
    for line in text.lines() {
        // 端末の色指定 (ESC [ ... m) を除く。
        let mut plain = String::with_capacity(line.len());
        let mut chars = line.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                plain.push(c);
            }
        }
        let plain = plain.trim_end();
        if plain.is_empty() {
            continue;
        }
        if plain.contains("PANIC") || plain.contains("panicked") || plain.contains("Exception") {
            log::error!("device text", "{plain}");
        } else {
            log::info!("device text", "{plain}");
        }
    }
}

type Port = Box<dyn serialport::SerialPort>;

pub struct SerialDevice {
    requested: Option<String>,
    trim_file: PitchTrimFile,
    port: Option<(String, Port)>,
    frames: Frames,
    events: Vec<protocol::Event>,
    nonce: u32,
    last_seq: Option<u32>,
    retry_at: Option<Instant>,
}

impl SerialDevice {
    pub fn new(requested: Option<String>, trim_file: PitchTrimFile) -> Self {
        Self {
            requested,
            trim_file,
            port: None,
            frames: Frames::default(),
            events: Vec::new(),
            // 起動ごとに異なる初期値とし、前回の daemon の応答と取り違えないようにする。
            nonce: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(1, |elapsed| elapsed.subsec_nanos() | 1),
            last_seq: None,
            retry_at: None,
        }
    }

    fn queue_event(&mut self, event: protocol::Event) {
        log::trace!("device", "受信 {event:?}");
        if self.events.len() == MAX_PENDING_EVENTS {
            log::warning!(
                "device",
                "取り出されない Event が溢れたため、古いものを捨てます"
            );
            self.events.remove(0);
        }
        self.events.push(event);
    }

    /// 要求を送り、Event 以外の応答を待つ。途中で届いた Event は保持する。
    fn exchange(&mut self, message: &protocol::Message) -> Result<protocol::Reply, String> {
        let (name, port) = self.port.as_mut().ok_or("未接続です")?;
        let mut buffer = [0; protocol::MAX_FRAME_BYTES];
        let frame = protocol::encode(message, &mut buffer).map_err(|e| e.to_string())?;
        log::trace!("device", "送信 {message:?}");
        port.set_timeout(WRITE_TIMEOUT)
            .and_then(|()| port.write_all(frame).map_err(Into::into))
            .and_then(|()| port.flush().map_err(Into::into))
            .and_then(|()| port.set_timeout(READ_TIMEOUT))
            .map_err(|e| format!("{name}: {e}"))?;
        let deadline = Instant::now() + REPLY_TIMEOUT;
        let mut chunk = [0; 256];
        while Instant::now() < deadline {
            let (name, port) = self.port.as_mut().ok_or("未接続です")?;
            let count = match port.read(&mut chunk) {
                Ok(0) => return Err(format!("{name}: 応答がありません")),
                Ok(count) => count,
                Err(error) if error.kind() == std::io::ErrorKind::TimedOut => continue,
                Err(error) => return Err(format!("{name}: {error}")),
            };
            let mut reply = None;
            for &byte in &chunk[..count] {
                match self.frames.push(byte) {
                    Some(Received::Reply(protocol::Reply::Event(event))) => self.queue_event(event),
                    Some(Received::Text(text)) => log_device_text(&text),
                    // 応答の後に続く byte は次の読取りで扱えるよう、ここで読み切る。
                    Some(Received::Reply(other)) if reply.is_none() => reply = Some(other),
                    Some(Received::Reply(other)) => {
                        log::warning!("device", "要求と対応しない応答を捨てました: {other:?}")
                    }
                    None => {}
                }
            }
            if let Some(reply) = reply {
                log::trace!("device", "受信 {reply:?}");
                return Ok(reply);
            }
        }
        if let Some(text) = self.frames.take_text() {
            log_device_text(&text);
        }
        Err("応答待ちがタイムアウトしました".into())
    }

    fn connect(&mut self, now: Instant) -> Result<(), String> {
        if self.retry_at.is_some_and(|at| now < at) {
            return Err("再接続を待っています".into());
        }
        self.retry_at = Some(now + RECONNECT_INTERVAL);
        let (name, port) = crate::open_port(self.requested.clone()).map_err(|e| e.to_string())?;
        self.port = Some((name.clone(), port));
        self.frames = Frames::default();
        let result = self.handshake();
        if let Err(error) = result {
            self.port = None;
            return Err(format!("{name}: {error}"));
        }
        log::info!("device", "{name} に接続しました");
        self.retry_at = None;
        Ok(())
    }

    fn handshake(&mut self) -> Result<(), String> {
        self.nonce = self.nonce.wrapping_add(1);
        let reply = self.exchange(&protocol::Message::Ping { nonce: self.nonce })?;
        crate::validate_pong(&reply, self.nonce)?;
        self.last_seq = None;
        self.apply_trim()
    }

    fn apply_trim(&mut self) -> Result<(), String> {
        let Some(trim) = crate::load_pitch_trim(&self.trim_file).map_err(|e| e.to_string())? else {
            return Ok(());
        };
        let seq = acked(self.exchange(&protocol::Message::PitchTrim(trim)))?;
        self.last_seq = Some(seq);
        Ok(())
    }

    fn disconnect(&mut self, now: Instant, error: &str) {
        if let Some(text) = self.frames.take_text() {
            log_device_text(&text);
        }
        if let Some((name, _)) = self.port.take() {
            log::warning!("device", "{name} との通信に失敗しました: {error}");
        }
        self.retry_at = Some(now + RECONNECT_INTERVAL);
    }
}

fn acked(reply: Result<protocol::Reply, String>) -> Result<u32, String> {
    match reply? {
        protocol::Reply::Ack { seq } => Ok(seq),
        protocol::Reply::Rejected { count } => Err(format!("拒否されました: rejected={count}")),
        other => Err(format!("Ack 以外の応答を受信しました: {other:?}")),
    }
}

/// 通し番号が進まずに巻き戻った場合、firmware が再起動したとみなす。
/// `u32::MAX` の次は 0 に戻るため、その直後の小さな値は巻戻りとしない。
fn restarted(previous: Option<u32>, seq: u32) -> bool {
    previous.is_some_and(|previous| seq <= previous && !(previous > u32::MAX - 16 && seq < 16))
}

impl Device for SerialDevice {
    fn send(&mut self, message: &protocol::Message, now: Instant) -> Result<Delivery, String> {
        let mut delivery = Delivery::Delivered;
        if self.port.is_none() {
            self.connect(now)?;
            delivery = Delivery::Reset;
        }
        let reply = self.exchange(message);
        if let Err(error) = &reply {
            self.disconnect(now, error);
        }
        let seq = acked(reply)?;
        if restarted(self.last_seq, seq) {
            log::warning!("device", "firmware の再起動を検出しました (seq {seq})");
            self.last_seq = Some(seq);
            // 要求自体は受理されている。表示状態は失われているため、trim の成否に
            // かかわらず Reset を返して全 slot を送り直させる。trim を適用できなかった
            // 場合は接続を切り、再接続の手順 (trim の適用を含む) でやり直す。
            if let Err(error) = self.apply_trim() {
                self.disconnect(now, &format!("pitch trim を再適用できません: {error}"));
            }
            return Ok(Delivery::Reset);
        }
        self.last_seq = Some(seq);
        Ok(delivery)
    }

    fn poll(&mut self) -> Vec<protocol::Event> {
        if let Some((_, port)) = self.port.as_mut()
            && port.bytes_to_read().is_ok_and(|available| available > 0)
            && port.set_timeout(POLL_TIMEOUT).is_ok()
        {
            let mut chunk = [0; 256];
            if let Ok(count) = port.read(&mut chunk) {
                for &byte in &chunk[..count] {
                    match self.frames.push(byte) {
                        Some(Received::Reply(protocol::Reply::Event(event))) => {
                            self.queue_event(event)
                        }
                        Some(Received::Text(text)) => log_device_text(&text),
                        Some(Received::Reply(other)) => {
                            log::warning!("device", "要求の無い応答を捨てました: {other:?}")
                        }
                        None => {}
                    }
                }
            }
        }
        std::mem::take(&mut self.events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire(reply: &protocol::Reply) -> Vec<u8> {
        let mut buffer = [0; protocol::MAX_FRAME_BYTES];
        protocol::encode(reply, &mut buffer).unwrap().to_vec()
    }

    #[test]
    fn sequence_regression_means_restart_except_wraparound() {
        assert!(!restarted(None, 1));
        assert!(!restarted(Some(1), 2));
        assert!(restarted(Some(5), 1));
        assert!(restarted(Some(5), 5));
        assert!(!restarted(Some(u32::MAX), 0));
        assert!(!restarted(Some(u32::MAX - 1), 3));
        assert!(restarted(Some(u32::MAX - 100), 0));
    }

    #[test]
    fn frames_separate_events_from_replies_and_resynchronize() {
        let event = protocol::Reply::Event(protocol::Event::Tap {
            slot: Some(protocol::Slot::BannerTop),
            card: Some(9),
            action: Some(1),
        });
        let ack = protocol::Reply::Ack { seq: 3 };
        let mut stream = vec![b'l', b'o', b'g', 0, 0];
        stream.extend(vec![0xFF; protocol::MAX_FRAME_BYTES + 3]);
        stream.push(0);
        stream.extend(wire(&event));
        stream.extend(wire(&ack));
        let mut frames = Frames::default();
        let decoded: Vec<_> = stream.iter().filter_map(|&b| frames.push(b)).collect();
        assert_eq!(
            decoded,
            [
                Received::Text("log".into()),
                Received::Reply(event),
                Received::Reply(ack)
            ]
        );
    }

    #[test]
    fn device_text_is_separated_from_binary_frames() {
        // COBS のフレームは制御バイトを含むため、テキストとはみなさない。
        let ack = wire(&protocol::Reply::Ack { seq: 1 });
        assert_eq!(as_text(&ack[..ack.len() - 1]), None);
        assert_eq!(
            as_text("\x1b[31m PANIC \r\n".as_bytes()).as_deref(),
            Some("\x1b[31m PANIC \r\n")
        );
        assert_eq!(as_text(&[0xFF, b'a']), None);

        // 区切りの無い panic の出力は take_text で取り出せる。
        let mut frames = Frames::default();
        for &byte in b"panicked at renderer.rs" {
            assert_eq!(frames.push(byte), None);
        }
        assert_eq!(
            frames.take_text().as_deref(),
            Some("panicked at renderer.rs")
        );
        assert_eq!(frames.take_text(), None);

        // 上限を超える長いテキストは区切りを待たずに出す。
        let long = vec![b'x'; protocol::MAX_FRAME_BYTES + 10];
        let texts: Vec<_> = long.iter().filter_map(|&b| frames.push(b)).collect();
        assert_eq!(texts.len(), 1);
        assert_eq!(frames.take_text().map(|t| t.len()), Some(11));
    }
}
