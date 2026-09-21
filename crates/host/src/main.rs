//! PC 側 CLI。
//!
//! 現段階では port の列挙と疎通確認のみを提供する。表示内容の組み立て (Card) と
//! 使用量の取得 (collector) は docs/design.md のフェーズに従って追加する。

use std::io::{Read, Write};
use std::time::Duration;

use clap::{Parser, Subcommand};

/// Espressif の USB Serial/JTAG が名乗る VID:PID。port の自動検出に使う。
const ESP_USB_SERIAL_JTAG: (u16, u16) = (0x303A, 0x1001);

#[derive(Parser)]
#[command(name = "stackchan", version, about = "my-stackchan host CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 接続されているシリアル port を列挙する
    ListPorts,
    /// 疎通確認を送り、応答を待つ
    Ping {
        /// port 名。省略時は VID:PID から自動検出する
        #[arg(long)]
        port: Option<String>,
    },
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    match cli.command {
        Command::ListPorts => list_ports(),
        Command::Ping { port } => ping(port),
    }
}

fn list_ports() -> Result<(), Box<dyn std::error::Error>> {
    for p in serialport::available_ports()? {
        match p.port_type {
            serialport::SerialPortType::UsbPort(info) => {
                println!(
                    "{}\tusb {:04x}:{:04x}\t{}",
                    p.port_name,
                    info.vid,
                    info.pid,
                    info.product.unwrap_or_default()
                );
            }
            other => println!("{}\t{:?}", p.port_name, other),
        }
    }
    Ok(())
}

fn detect_port() -> Result<String, Box<dyn std::error::Error>> {
    let found = serialport::available_ports()?.into_iter().find(|p| {
        matches!(
            &p.port_type,
            serialport::SerialPortType::UsbPort(info)
                if (info.vid, info.pid) == ESP_USB_SERIAL_JTAG
        )
    });
    found
        .map(|p| p.port_name)
        .ok_or_else(|| "USB Serial/JTAG (303a:1001) の port が見つかりません".into())
}

fn ping(port: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    let port_name = match port {
        Some(p) => p,
        None => detect_port()?,
    };
    let mut port = serialport::new(&port_name, 115_200)
        .timeout(Duration::from_millis(1000))
        .open()?;

    let nonce = 0x5A5A_1234;
    let mut buf = [0u8; 32];
    let frame = protocol::encode(&protocol::Message::Ping { nonce }, &mut buf)?;
    port.write_all(frame)?;
    port.flush()?;

    let mut rx = [0u8; protocol::MAX_FRAME_BYTES];
    let mut len = 0;
    loop {
        let n = port.read(&mut rx[len..])?;
        if n == 0 {
            return Err("応答がありません".into());
        }
        len += n;
        if let Some(end) = rx[..len].iter().position(|&b| b == 0) {
            let reply: protocol::Reply = protocol::decode(&mut rx[..=end])?;
            println!("{port_name}: {reply:?}");
            return validate_pong(&reply, nonce).map_err(Into::into);
        }
        if len == rx.len() {
            return Err("フレームが長すぎます".into());
        }
    }
}

fn validate_pong(reply: &protocol::Reply, expected_nonce: u32) -> Result<(), String> {
    match reply {
        protocol::Reply::Pong { nonce, .. } if *nonce != expected_nonce => {
            Err("nonce が一致しません".into())
        }
        protocol::Reply::Pong { version, .. } if *version != protocol::VERSION => Err(format!(
            "プロトコル版が一致しません: host={}, firmware={version}",
            protocol::VERSION
        )),
        protocol::Reply::Pong { .. } => Ok(()),
        _ => Err("Ping に対して Pong 以外の応答を受信しました".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONCE: u32 = 0x5A5A_1234;

    #[test]
    fn accepts_matching_nonce_and_version() {
        let reply = protocol::Reply::Pong {
            nonce: NONCE,
            version: protocol::VERSION,
        };
        assert_eq!(validate_pong(&reply, NONCE), Ok(()));
    }

    #[test]
    fn rejects_incompatible_version_even_with_matching_nonce() {
        let version = protocol::VERSION.wrapping_add(1);
        let reply = protocol::Reply::Pong {
            nonce: NONCE,
            version,
        };
        assert_eq!(
            validate_pong(&reply, NONCE),
            Err(format!(
                "プロトコル版が一致しません: host={}, firmware={version}",
                protocol::VERSION
            ))
        );
    }

    #[test]
    fn rejects_wrong_nonce_even_with_matching_version() {
        let reply = protocol::Reply::Pong {
            nonce: NONCE + 1,
            version: protocol::VERSION,
        };
        assert_eq!(
            validate_pong(&reply, NONCE),
            Err("nonce が一致しません".into())
        );
    }

    #[test]
    fn rejects_non_pong_replies() {
        for reply in [
            protocol::Reply::Ack { seq: 0 },
            protocol::Reply::Rejected { count: 0 },
        ] {
            assert_eq!(
                validate_pong(&reply, NONCE),
                Err("Ping に対して Pong 以外の応答を受信しました".into())
            );
        }
    }
}
