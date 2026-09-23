//! plugin を Rust で書くための補助。
//!
//! stdout はフレーム専用である。plugin は `println!` 等で stdout に書いてはならず、
//! 診断出力には stderr か `PluginMessage::Log` を使う。
//!
//! ```no_run
//! use std::time::Duration;
//! use plugin_api::{API_VERSION, Capabilities, Hello, HostMessage, client};
//!
//! let hello = Hello {
//!     api_version: API_VERSION,
//!     name: "example".into(),
//!     version: env!("CARGO_PKG_VERSION").into(),
//!     capabilities: Capabilities { cards: 1, ..Capabilities::default() },
//! };
//! let mut connection = client::connect_stdio(hello)?;
//! loop {
//!     match connection.recv_timeout(Duration::from_secs(1))? {
//!         Some(HostMessage::Shutdown) => break,
//!         _ if connection.is_closed() => break,
//!         // 定期的な処理と、Action 等への応答をここで行う。
//!         _ => {}
//!     }
//! }
//! # Ok::<(), client::ClientError>(())
//! ```

use std::io::{self, Read, Write};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use crate::{API_VERSION, FrameError, FrameReader, Hello, HostMessage, Init, PluginMessage};

#[derive(Debug)]
pub enum ClientError {
    Frame(FrameError),
    /// daemon の応答が手順に反する (Init の前に別のメッセージが来た等)。
    Protocol(&'static str),
    /// 送信しようとしたメッセージが検証に失敗した。
    Invalid(&'static str),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Frame(error) => write!(f, "{error}"),
            Self::Protocol(reason) | Self::Invalid(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for ClientError {}

impl From<FrameError> for ClientError {
    fn from(error: FrameError) -> Self {
        Self::Frame(error)
    }
}

impl From<io::Error> for ClientError {
    fn from(error: io::Error) -> Self {
        Self::Frame(error.into())
    }
}

/// daemon との接続。受信は専用スレッドが行い、`recv_timeout` で取り出す。
pub struct Connection<W> {
    writer: W,
    incoming: Receiver<Result<HostMessage, FrameError>>,
    init: Init,
    closed: bool,
}

/// stdin / stdout で daemon に接続する。
pub fn connect_stdio(hello: Hello) -> Result<Connection<io::Stdout>, ClientError> {
    connect(io::stdin(), io::stdout(), hello)
}

/// `Hello` を送り、`Init` を受け取るまで待つ。
pub fn connect<R, W>(reader: R, mut writer: W, hello: Hello) -> Result<Connection<W>, ClientError>
where
    R: Read + Send + 'static,
    W: Write,
{
    let hello = PluginMessage::Hello(hello);
    hello.validate().map_err(ClientError::Invalid)?;
    crate::write_frame(&mut writer, &hello)?;

    let (sender, incoming) = mpsc::channel();
    thread::spawn(move || {
        let mut frames = FrameReader::new(reader);
        loop {
            let item = match frames.read_frame::<HostMessage>() {
                Ok(Some(message)) => Ok(message),
                Ok(None) => break,
                Err(error) => Err(error),
            };
            let fatal = matches!(item, Err(FrameError::Io(_)));
            if sender.send(item).is_err() || fatal {
                break;
            }
        }
    });

    let init = match incoming.recv() {
        Ok(Ok(HostMessage::Init(init))) => init,
        Ok(Ok(_)) => {
            return Err(ClientError::Protocol(
                "Init より前に別のメッセージを受信しました",
            ));
        }
        Ok(Err(error)) => return Err(error.into()),
        Err(_) => return Err(ClientError::Protocol("Init を受信する前に接続が閉じました")),
    };
    if init.api_version != API_VERSION {
        return Err(ClientError::Protocol("plugin API の版が一致しません"));
    }
    Ok(Connection {
        writer,
        incoming,
        init,
        closed: false,
    })
}

impl<W: Write> Connection<W> {
    pub fn init(&self) -> &Init {
        &self.init
    }

    /// `Init` の設定値を名前で引く。
    pub fn param(&self, key: &str) -> Option<&str> {
        self.init
            .params
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    /// 送信前に `PluginMessage::validate` を通す。daemon の拒否を待たずに誤りを検出するため。
    pub fn send(&mut self, message: &PluginMessage) -> Result<(), ClientError> {
        message.validate().map_err(ClientError::Invalid)?;
        crate::write_frame(&mut self.writer, message)?;
        Ok(())
    }

    /// daemon からのメッセージを最大 `timeout` 待つ。時間内に無ければ `Ok(None)`。
    /// 接続が閉じた後も `Ok(None)` を返し、`is_closed` が真になる。
    pub fn recv_timeout(&mut self, timeout: Duration) -> Result<Option<HostMessage>, ClientError> {
        if self.closed {
            return Ok(None);
        }
        match self.incoming.recv_timeout(timeout) {
            Ok(Ok(message)) => Ok(Some(message)),
            Ok(Err(error)) => Err(error.into()),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => {
                self.closed = true;
                Ok(None)
            }
        }
    }

    /// daemon 側の出力が閉じたか。閉じた後は終了してよい。
    pub fn is_closed(&self) -> bool {
        self.closed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Capabilities, Limits};

    fn hello() -> Hello {
        Hello {
            api_version: API_VERSION,
            name: "test".into(),
            version: "0".into(),
            capabilities: Capabilities {
                cards: 1,
                ..Capabilities::default()
            },
        }
    }

    fn init() -> Init {
        Init {
            api_version: API_VERSION,
            granted: Capabilities {
                cards: 1,
                ..Capabilities::default()
            },
            params: vec![("utc_offset_minutes".into(), "540".into())],
            limits: Limits {
                banner_rows: 2,
                overlay_rows: 11,
                row_elements: 2,
                text_bytes: 48,
                bar_label_bytes: 12,
                notify_bytes: 512,
                presence_detail_bytes: 20,
            },
        }
    }

    #[test]
    fn handshake_delivers_init_and_later_messages() {
        let (host_reader, plugin_writer) = io::pipe().unwrap();
        let (plugin_reader, mut host_writer) = io::pipe().unwrap();
        crate::write_frame(&mut host_writer, &HostMessage::Init(init())).unwrap();
        let mut connection = connect(plugin_reader, plugin_writer, hello()).unwrap();
        assert_eq!(connection.param("utc_offset_minutes"), Some("540"));

        let mut from_plugin = FrameReader::new(host_reader);
        assert_eq!(
            from_plugin.read_frame::<PluginMessage>().unwrap(),
            Some(PluginMessage::Hello(hello()))
        );

        crate::write_frame(&mut host_writer, &HostMessage::Shutdown).unwrap();
        assert_eq!(
            connection.recv_timeout(Duration::from_secs(5)).unwrap(),
            Some(HostMessage::Shutdown)
        );
        drop(host_writer);
        while !connection.is_closed() {
            assert!(
                connection
                    .recv_timeout(Duration::from_secs(5))
                    .unwrap()
                    .is_none()
            );
        }
    }

    #[test]
    fn message_before_init_and_version_mismatch_are_rejected() {
        let (_host_reader, plugin_writer) = io::pipe().unwrap();
        let (plugin_reader, mut host_writer) = io::pipe().unwrap();
        crate::write_frame(&mut host_writer, &HostMessage::Shutdown).unwrap();
        assert!(matches!(
            connect(plugin_reader, plugin_writer, hello()),
            Err(ClientError::Protocol(_))
        ));

        let (_host_reader, plugin_writer) = io::pipe().unwrap();
        let (plugin_reader, mut host_writer) = io::pipe().unwrap();
        let mut newer = init();
        newer.api_version = API_VERSION + 1;
        crate::write_frame(&mut host_writer, &HostMessage::Init(newer)).unwrap();
        assert!(matches!(
            connect(plugin_reader, plugin_writer, hello()),
            Err(ClientError::Protocol(_))
        ));
    }

    #[test]
    fn invalid_messages_are_not_written() {
        let (plugin_reader, mut host_writer) = io::pipe().unwrap();
        crate::write_frame(&mut host_writer, &HostMessage::Init(init())).unwrap();
        let mut connection = connect(plugin_reader, Vec::new(), hello()).unwrap();
        let written = connection.writer.len();
        let invalid = PluginMessage::CardRemove {
            card: crate::MAX_CARDS,
        };
        assert!(matches!(
            connection.send(&invalid),
            Err(ClientError::Invalid(_))
        ));
        assert_eq!(connection.writer.len(), written);
    }
}
