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
            return match reply {
                protocol::Reply::Pong { nonce: n, .. } if n == nonce => Ok(()),
                _ => Err("nonce が一致しません".into()),
            };
        }
        if len == rx.len() {
            return Err("フレームが長すぎます".into());
        }
    }
}
