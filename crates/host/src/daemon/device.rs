//! daemon が保持するデバイスとの接続。
//!
//! 単発の CLI と異なり接続を保ち続けるため、次を扱う。
//! - 切断後の再接続と、接続時の版の照合と pitch trim の再適用
//! - Ping の nonce を送信ごとに変え、再接続前の古い応答と区別する
//! - Ack の通し番号の巻戻りから firmware の再起動を検出する

use std::time::{Duration, Instant};

use crate::PitchTrimFile;

/// 切断後、次に接続を試みるまでの間隔。
const RECONNECT_INTERVAL: Duration = Duration::from_secs(2);

/// 送信の結果。`Reset` はデバイスの表示状態が失われたことを表し、daemon は全 slot を送り直す。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    Delivered,
    Reset,
}

pub trait Device {
    fn send(&mut self, message: &protocol::Message, now: Instant) -> Result<Delivery, String>;
}

type Port = Box<dyn serialport::SerialPort>;

pub struct SerialDevice {
    requested: Option<String>,
    trim_file: PitchTrimFile,
    port: Option<(String, Port)>,
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
            // 起動ごとに異なる初期値とし、前回の daemon の応答と取り違えないようにする。
            nonce: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(1, |elapsed| elapsed.subsec_nanos() | 1),
            last_seq: None,
            retry_at: None,
        }
    }

    fn connect(&mut self, now: Instant) -> Result<(), String> {
        if self.retry_at.is_some_and(|at| now < at) {
            return Err("再接続を待っています".into());
        }
        self.retry_at = Some(now + RECONNECT_INTERVAL);
        let (name, mut port) =
            crate::open_port(self.requested.clone()).map_err(|e| e.to_string())?;
        self.nonce = self.nonce.wrapping_add(1);
        let reply = crate::exchange(&mut port, &protocol::Message::Ping { nonce: self.nonce })
            .map_err(|e| format!("{name}: {e}"))?;
        crate::validate_pong(&reply, self.nonce).map_err(|e| format!("{name}: {e}"))?;
        self.last_seq = None;
        if let Some(trim) = crate::load_pitch_trim(&self.trim_file).map_err(|e| e.to_string())? {
            let seq = acked(crate::exchange(
                &mut port,
                &protocol::Message::PitchTrim(trim),
            ))?;
            self.last_seq = Some(seq);
        }
        eprintln!("[daemon] {name} に接続しました");
        self.port = Some((name, port));
        self.retry_at = None;
        Ok(())
    }

    fn apply_trim(&mut self) -> Result<(), String> {
        let Some(trim) = crate::load_pitch_trim(&self.trim_file).map_err(|e| e.to_string())? else {
            return Ok(());
        };
        let (_, port) = self.port.as_mut().ok_or("未接続です")?;
        let seq = acked(crate::exchange(port, &protocol::Message::PitchTrim(trim)))?;
        self.last_seq = Some(seq);
        Ok(())
    }
}

fn acked(reply: Result<protocol::Reply, Box<dyn std::error::Error>>) -> Result<u32, String> {
    match reply.map_err(|e| e.to_string())? {
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
        let (name, port) = self.port.as_mut().ok_or("未接続です")?;
        let reply = crate::exchange(port, message);
        if let Err(error) = &reply {
            eprintln!("[daemon] {name} との通信に失敗しました: {error}");
            self.port = None;
            self.retry_at = Some(now + RECONNECT_INTERVAL);
        }
        let seq = acked(reply)?;
        if restarted(self.last_seq, seq) {
            eprintln!("[daemon] firmware の再起動を検出しました (seq {seq})");
            self.last_seq = Some(seq);
            self.apply_trim()?;
            return Ok(Delivery::Reset);
        }
        self.last_seq = Some(seq);
        Ok(delivery)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
