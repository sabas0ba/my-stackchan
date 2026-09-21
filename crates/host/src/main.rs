//! PC 側 CLI。
//!
//! 現段階では port の列挙と疎通確認のみを提供する。表示内容の組み立て (Card) と
//! 使用量の取得 (collector) は docs/design.md のフェーズに従って追加する。

use std::io::{Read, Write};
use std::time::{Duration, Instant};

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
    // 書き込み直後に残る bootloader のログを今回の応答に混入させない。
    port.clear(serialport::ClearBuffer::Input)?;

    let nonce = 0x5A5A_1234;
    let mut buf = [0u8; 32];
    let frame = protocol::encode(&protocol::Message::Ping { nonce }, &mut buf)?;
    port.write_all(frame)?;
    port.flush()?;

    let reply = read_reply(&mut port)?;
    println!("{port_name}: {reply:?}");
    validate_pong(&reply, nonce).map_err(Into::into)
}

fn read_reply(reader: &mut impl Read) -> Result<protocol::Reply, Box<dyn std::error::Error>> {
    let mut rx = [0u8; protocol::MAX_FRAME_BYTES];
    let mut len = 0;
    let mut discarding = false;
    let deadline = Instant::now() + Duration::from_secs(1);
    // 起動ログや空の区切りを読み飛ばしても、連続入力で永久に待たない。
    while Instant::now() < deadline {
        let mut byte = [0];
        let n = reader.read(&mut byte)?;
        if n == 0 {
            return Err("応答がありません".into());
        }
        if byte[0] == 0 {
            if !discarding && len != 0 {
                rx[len] = 0;
                if let Ok(reply) = protocol::decode(&mut rx[..=len]) {
                    return Ok(reply);
                }
            }
            len = 0;
            discarding = false;
        } else if !discarding {
            if len == rx.len() - 1 {
                discarding = true;
            } else {
                rx[len] = byte[0];
                len += 1;
            }
        }
    }
    Err("応答待ちがタイムアウトしました".into())
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

    #[test]
    fn reply_reader_resynchronizes_after_boot_log_and_oversized_frame() {
        let reply = protocol::Reply::Pong {
            nonce: NONCE,
            version: protocol::VERSION,
        };
        let mut buffer = [0; 32];
        let frame = protocol::encode(&reply, &mut buffer).unwrap();
        let mut stream = b"boot: Loaded app\r\n\0\0".to_vec();
        stream.extend([1; protocol::MAX_FRAME_BYTES]);
        // 上限超過フレームの末尾にある正しい応答も、区切りまでは捨てる。
        stream.extend(frame);
        stream.extend(frame);
        assert_eq!(read_reply(&mut stream.as_slice()).unwrap(), reply);
    }

    #[test]
    fn reply_reader_rejects_truncated_input() {
        let mut stream = &[1, 6, 1, 2][..];
        assert!(read_reply(&mut stream).is_err());
    }

    #[test]
    #[ignore = "Ping 対応 firmware の実機と STACKCHAN_TEST_PORT が必要"]
    fn hardware_ping_and_frame_recovery() {
        let port_name = std::env::var("STACKCHAN_TEST_PORT").expect("STACKCHAN_TEST_PORT が必要");
        let mut port = serialport::new(port_name, 115_200)
            .timeout(Duration::from_secs(2))
            .open()
            .unwrap();
        port.clear(serialport::ClearBuffer::Input).unwrap();
        let mut buffer = [0; 32];
        let ping =
            protocol::encode(&protocol::Message::Ping { nonce: NONCE }, &mut buffer).unwrap();

        // USB パケットが分割されても終端まで蓄積できることを実機で確認する。
        for byte in ping {
            port.write_all(&[*byte]).unwrap();
            port.flush().unwrap();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            validate_pong(&read_reply(&mut port).unwrap(), NONCE),
            Ok(())
        );

        port.write_all(&[0xFF, 1, 2, 0]).unwrap();
        port.flush().unwrap();
        let protocol::Reply::Rejected { count } = read_reply(&mut port).unwrap() else {
            panic!("不正フレームが拒否されませんでした");
        };
        assert!(count > 0);

        // 上限超過フレームの末尾が正しい Ping でも、その区切りまでは破棄する。
        port.write_all(&[1; protocol::MAX_FRAME_BYTES * 2]).unwrap();
        port.write_all(ping).unwrap();
        port.flush().unwrap();
        assert_eq!(
            read_reply(&mut port).unwrap(),
            protocol::Reply::Rejected {
                count: count.saturating_add(1)
            }
        );

        // 連結されたフレームを順番どおり返し、拒否後にも正常に復帰する。
        port.write_all(ping).unwrap();
        port.write_all(ping).unwrap();
        port.flush().unwrap();
        for _ in 0..2 {
            assert_eq!(
                validate_pong(&read_reply(&mut port).unwrap(), NONCE),
                Ok(())
            );
        }
    }
}
