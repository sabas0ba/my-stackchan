//! plugin から外部の機器への接続。
//!
//! plugin ごとのコンテナ分離では plugin のコンテナがネットワークを持たず、利用者設定の
//! `net.allow` に列挙した宛先へは、daemon が `Init` で知らせる Unix socket (中継) を
//! 介してだけ接続できる。分離をしていない場合は直接接続する。plugin の実装を分離の
//! 有無で分けないよう、切替えを本 module が行う。
//!
//! 返す `Stream` は暗号化されていない byte 列の経路である。TLS は plugin がこの上で張る。

use std::io::{self, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::time::Duration;

use crate::Endpoint;

/// 宛先との接続。中継を介す場合と直接の場合で型を分けないために包む。
#[derive(Debug)]
pub enum Stream {
    Tcp(TcpStream),
    #[cfg(unix)]
    Unix(UnixStream),
}

impl Stream {
    pub fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            Self::Tcp(stream) => stream.set_read_timeout(timeout),
            #[cfg(unix)]
            Self::Unix(stream) => stream.set_read_timeout(timeout),
        }
    }

    pub fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        match self {
            Self::Tcp(stream) => stream.set_write_timeout(timeout),
            #[cfg(unix)]
            Self::Unix(stream) => stream.set_write_timeout(timeout),
        }
    }

    pub fn shutdown(&self, how: Shutdown) -> io::Result<()> {
        match self {
            Self::Tcp(stream) => stream.shutdown(how),
            #[cfg(unix)]
            Self::Unix(stream) => stream.shutdown(how),
        }
    }

    pub fn try_clone(&self) -> io::Result<Self> {
        Ok(match self {
            Self::Tcp(stream) => Self::Tcp(stream.try_clone()?),
            #[cfg(unix)]
            Self::Unix(stream) => Self::Unix(stream.try_clone()?),
        })
    }
}

impl Read for Stream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self {
            Self::Tcp(stream) => stream.read(buffer),
            #[cfg(unix)]
            Self::Unix(stream) => stream.read(buffer),
        }
    }
}

impl Write for Stream {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        match self {
            Self::Tcp(stream) => stream.write(buffer),
            #[cfg(unix)]
            Self::Unix(stream) => stream.write(buffer),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Tcp(stream) => stream.flush(),
            #[cfg(unix)]
            Self::Unix(stream) => stream.flush(),
        }
    }
}

/// `addr` (`<IP アドレス>:<port>`) へ接続する。
///
/// `endpoints` に同じ宛先があれば、その Unix socket (中継) へ接続する。無ければ TCP で
/// 直接接続する。名前解決は行わない。`timeout` は直接の接続の確立にだけ適用する
/// (中継は宛先への接続を自身で行い、失敗した場合は接続を閉じる)。
pub fn connect(endpoints: &[Endpoint], addr: &str, timeout: Duration) -> io::Result<Stream> {
    let target: SocketAddr = addr.parse().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "接続先は <IP アドレス>:<port> で指定します",
        )
    })?;
    // 表記の揺れ (IPv6 の省略形等) で一致を逃さないよう、解析した値で比べる。
    let relayed = endpoints
        .iter()
        .find(|endpoint| endpoint.addr.parse::<SocketAddr>().ok() == Some(target));
    match relayed {
        #[cfg(unix)]
        Some(endpoint) => UnixStream::connect(&endpoint.path).map(Stream::Unix),
        #[cfg(not(unix))]
        Some(_) => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "中継の Unix socket はこの OS では使えません",
        )),
        None => TcpStream::connect_timeout(&target, timeout).map(Stream::Tcp),
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;

    use super::*;

    const TIMEOUT: Duration = Duration::from_secs(5);

    #[test]
    fn unlisted_destination_is_reached_directly() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let mut stream = connect(&[], &addr, TIMEOUT).unwrap();
        assert!(matches!(stream, Stream::Tcp(_)));
        let (mut peer, _) = listener.accept().unwrap();
        stream.write_all(b"direct").unwrap();
        let mut received = [0u8; 6];
        peer.read_exact(&mut received).unwrap();
        assert_eq!(&received, b"direct");
    }

    #[test]
    fn names_and_malformed_destinations_are_rejected_without_resolution() {
        for addr in [
            "printer.local:8883",
            "192.168.1.50",
            "192.168.1.50:99999",
            "",
        ] {
            let error = connect(&[], addr, TIMEOUT).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::InvalidInput, "{addr}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn listed_destination_goes_through_its_socket() {
        use std::os::unix::net::UnixListener;

        let dir = std::env::temp_dir().join(format!("stackchan-net-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("0.sock");
        let listener = UnixListener::bind(&path).unwrap();
        // 宛先は到達できない番地とし、直接の接続では成立しないことで中継の使用を確かめる。
        let endpoints = [Endpoint {
            addr: "[2001:db8::1]:8883".into(),
            path: path.to_str().unwrap().into(),
        }];
        // 省略しない表記でも同じ宛先として扱う。
        let mut stream = connect(&endpoints, "[2001:db8:0:0:0:0:0:1]:8883", TIMEOUT).unwrap();
        assert!(matches!(stream, Stream::Unix(_)));
        let (mut peer, _) = listener.accept().unwrap();
        stream.write_all(b"relayed").unwrap();
        let mut received = [0u8; 7];
        peer.read_exact(&mut received).unwrap();
        assert_eq!(&received, b"relayed");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
