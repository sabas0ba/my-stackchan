//! 許可した宛先への中継 (`stackchan relay`)。
//!
//! plugin ごとのコンテナ分離では plugin のコンテナがネットワークを持たない。利用者設定の
//! `net.allow` に列挙した宛先ごとに Unix socket で待ち受け、接続を宛先へ TCP で転送する。
//! 宛先は起動時の引数で固定され、plugin からは変えられない。内容は解釈しない
//! (TLS は plugin が張る)。設計は docs/plugin.md の「接続先の許可」を参照する。

use std::io;
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

/// 宛先ごとの同時接続数の上限。plugin が接続を開き続けて、中継のスレッドと宛先の機器の
/// 接続数を使い切らないようにする。
const MAX_CONNECTIONS: usize = 4;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub socket: PathBuf,
    pub target: SocketAddr,
}

/// `<socket の path>=<IP アドレス>:<port>` を解析する。path は `=` を含み得るため、
/// 最後の `=` で分ける (宛先は `=` を含まない)。
pub fn parse_route(text: &str) -> Result<Route, String> {
    let (socket, target) = text
        .rsplit_once('=')
        .ok_or("route は <socket の path>=<IP アドレス>:<port> で指定します")?;
    if socket.is_empty() {
        return Err("route の socket の path が空です".into());
    }
    let target = target
        .parse()
        .map_err(|_| format!("route の宛先が不正です: {target}"))?;
    Ok(Route {
        socket: socket.into(),
        target,
    })
}

/// すべての route で待ち受ける。待受けを開けない route があれば、何も中継せずに失敗する。
pub fn run(routes: Vec<Route>) -> io::Result<()> {
    let mut listeners = Vec::new();
    for route in routes {
        let listener = listen(&route)?;
        eprintln!(
            "relay: {} を {} へ中継します",
            route.socket.display(),
            route.target
        );
        listeners.push((listener, route.target));
    }
    let workers: Vec<_> = listeners
        .into_iter()
        .map(|(listener, target)| thread::spawn(move || serve(listener, target)))
        .collect();
    for worker in workers {
        let _ = worker.join();
    }
    Ok(())
}

fn listen(route: &Route) -> io::Result<UnixListener> {
    if let Some(parent) = route.socket.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // 前回の中継が残した socket があると bind できない。
    match std::fs::remove_file(&route.socket) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    UnixListener::bind(&route.socket)
}

fn serve(listener: UnixListener, target: SocketAddr) {
    let active = Arc::new(AtomicUsize::new(0));
    for stream in listener.incoming() {
        let Ok(stream) = stream else {
            continue;
        };
        if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
            active.fetch_sub(1, Ordering::SeqCst);
            eprintln!("relay: {target} への同時接続数が上限のため、接続を閉じました");
            continue;
        }
        let active = Arc::clone(&active);
        thread::spawn(move || {
            if let Err(error) = forward(stream, target) {
                eprintln!("relay: {target} へ中継できません: {error}");
            }
            active.fetch_sub(1, Ordering::SeqCst);
        });
    }
}

/// 1 本の接続を宛先へ転送する。どちらかの向きが終わったら両方を閉じる。片方向だけを
/// 閉じた接続が残り、同時接続数の枠を占め続けないようにするためである。
fn forward(plugin: UnixStream, target: SocketAddr) -> io::Result<()> {
    let device = TcpStream::connect_timeout(&target, CONNECT_TIMEOUT)?;
    // 中継で小さな書込みをまとめて遅らせない。まとめるかどうかは両端が決める。
    device.set_nodelay(true)?;
    let (mut plugin_reader, mut plugin_writer) = (plugin.try_clone()?, plugin);
    let (mut device_reader, mut device_writer) = (device.try_clone()?, device);
    let upstream = thread::spawn(move || {
        let _ = io::copy(&mut plugin_reader, &mut device_writer);
        let _ = device_writer.shutdown(Shutdown::Both);
        let _ = plugin_reader.shutdown(Shutdown::Both);
    });
    let _ = io::copy(&mut device_reader, &mut plugin_writer);
    let _ = plugin_writer.shutdown(Shutdown::Both);
    let _ = device_reader.shutdown(Shutdown::Both);
    let _ = upstream.join();
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::Path;
    use std::time::Instant;

    use super::*;

    const TIMEOUT: Duration = Duration::from_secs(5);

    /// 受信した内容に宛先の印を付けて返す TCP の相手。どの宛先へ届いたかを確かめる。
    fn echo_server(mark: u8) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                thread::spawn(move || {
                    let mut buffer = [0u8; 64];
                    while let Ok(length @ 1..) = stream.read(&mut buffer) {
                        let mut reply = vec![mark];
                        reply.extend_from_slice(&buffer[..length]);
                        if stream.write_all(&reply).is_err() {
                            break;
                        }
                    }
                });
            }
        });
        addr
    }

    struct Relay {
        dir: PathBuf,
    }

    impl Relay {
        fn start(name: &str, targets: &[SocketAddr]) -> Self {
            let dir =
                std::env::temp_dir().join(format!("stackchan-relay-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            for (index, target) in targets.iter().enumerate() {
                let route = Route {
                    socket: dir.join(format!("net/{index}.sock")),
                    target: *target,
                };
                let listener = listen(&route).unwrap();
                let target = route.target;
                thread::spawn(move || serve(listener, target));
            }
            Self { dir }
        }

        fn connect(&self, index: usize) -> UnixStream {
            let stream = UnixStream::connect(self.dir.join(format!("net/{index}.sock"))).unwrap();
            stream.set_read_timeout(Some(TIMEOUT)).unwrap();
            stream
        }
    }

    impl Drop for Relay {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn exchange(stream: &mut UnixStream, message: &[u8]) -> Vec<u8> {
        stream.write_all(message).unwrap();
        let mut reply = vec![0u8; message.len() + 1];
        stream.read_exact(&mut reply).unwrap();
        reply
    }

    fn is_closed(stream: &mut UnixStream) -> bool {
        matches!(stream.read(&mut [0u8; 1]), Ok(0))
    }

    #[test]
    fn route_is_split_at_the_last_separator() {
        assert_eq!(
            parse_route("/run/a=b/net/0.sock=192.168.1.50:8883").unwrap(),
            Route {
                socket: Path::new("/run/a=b/net/0.sock").into(),
                target: "192.168.1.50:8883".parse().unwrap(),
            }
        );
        assert!(parse_route("/run/net/0.sock=[fe80::1]:6000").is_ok());
        for bad in [
            "/run/net/0.sock",
            "=192.168.1.50:8883",
            "/run/net/0.sock=printer.local:8883",
            "/run/net/0.sock=192.168.1.50",
        ] {
            assert!(parse_route(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn each_socket_reaches_only_its_own_destination() {
        let relay = Relay::start("routes", &[echo_server(b'A'), echo_server(b'B')]);
        let mut first = relay.connect(0);
        let mut second = relay.connect(1);
        assert_eq!(exchange(&mut first, b"one"), b"Aone");
        assert_eq!(exchange(&mut second, b"two"), b"Btwo");
        assert_eq!(exchange(&mut first, b"again"), b"Aagain");
    }

    #[test]
    fn connections_beyond_the_limit_are_closed_until_one_ends() {
        let relay = Relay::start("limit", &[echo_server(b'A')]);
        let mut held: Vec<UnixStream> = (0..MAX_CONNECTIONS).map(|_| relay.connect(0)).collect();
        for stream in &mut held {
            assert_eq!(exchange(stream, b"x"), b"Ax");
        }
        assert!(is_closed(&mut relay.connect(0)), "上限を超えた接続");

        // 1 本を閉じると枠が空く。空くのは中継が終了を検出した後なので、成立するまで試す。
        drop(held.pop());
        let deadline = Instant::now() + TIMEOUT;
        loop {
            let mut stream = relay.connect(0);
            let mut reply = [0u8; 2];
            if stream.write_all(b"y").is_ok() && stream.read_exact(&mut reply).is_ok() {
                assert_eq!(&reply, b"Ay");
                break;
            }
            assert!(Instant::now() < deadline, "枠が空きません");
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn unreachable_destination_closes_the_connection() {
        // 待受けを閉じた直後の port は接続を拒否する。
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let target = listener.local_addr().unwrap();
        drop(listener);
        let relay = Relay::start("refused", &[target]);
        assert!(is_closed(&mut relay.connect(0)));
    }
}
