//! plugin のコンテナで動く起動補助 (`stackchan plugin-run`)。
//!
//! daemon の socket へ接続し、受け取った起動情報で plugin を子プロセスとして起動する。
//! socket をそのまま子プロセスの stdin / stdout とするため、plugin は従来どおり stdio で
//! フレームを交換するプログラムのままでよい。以降のフレームは本プロセスを経由しない。
//!
//! 再起動の待ち時間と無効化は daemon が判断する (本プロセスは plugin と同じコンテナで
//! 動くため、daemon は信頼しない)。本プロセスは一定の間隔で接続を試み、daemon が接続を
//! 受け付けない間は、起動情報を受け取る前に閉じられることで待つ。

use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::thread;
use std::time::Duration;

use crate::launch::{self, Launch};

/// 接続を試みる間隔。daemon は待ち時間内の接続を閉じるだけなので、短くても害は無い。
const RETRY_INTERVAL: Duration = Duration::from_secs(1);

pub fn run(socket: &Path) -> ! {
    loop {
        match serve(socket) {
            Ok(Some(status)) => eprintln!("plugin-run: plugin が終了しました ({status})"),
            // daemon が未起動か、接続を受け付けていない。頻繁に起きるため記録しない。
            Ok(None) => {}
            Err(error) => eprintln!("plugin-run: plugin を起動できません: {error}"),
        }
        thread::sleep(RETRY_INTERVAL);
    }
}

/// 1 回の接続を扱う。daemon が接続を受け付けなかった場合は `Ok(None)` を返す。
fn serve(socket: &Path) -> io::Result<Option<ExitStatus>> {
    let Ok(mut stream) = UnixStream::connect(socket) else {
        return Ok(None);
    };
    let Ok(launch) = launch::receive(&mut stream) else {
        return Ok(None);
    };
    spawn(stream, &launch, |name| std::env::var_os(name)).map(Some)
}

fn spawn(
    stream: UnixStream,
    (argv, env): &Launch,
    lookup: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> io::Result<ExitStatus> {
    let stdin = OwnedFd::from(stream.try_clone()?);
    let stdout = OwnedFd::from(stream);
    // stderr は引き継ぎ、コンテナエンジンのログに残す。
    Command::new(&argv[0])
        .args(&argv[1..])
        .env_clear()
        .envs(launch::child_env(env, lookup))
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .status()
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};

    use super::*;

    #[test]
    fn child_exchanges_bytes_with_the_peer_over_the_socket() {
        let (mut daemon_end, plugin_end) = UnixStream::pair().unwrap();
        let launch: Launch = (
            vec![
                "sh".into(),
                "-c".into(),
                // stdin の 1 行と、起動情報の環境変数を stdout へ返す。
                "read line; printf '%s:%s:%s' \"$line\" \"$NAME\" \"${LEAKED-unset}\"".into(),
            ],
            vec![("NAME".into(), "value".into())],
        );
        let child = thread::spawn(move || {
            spawn(plugin_end, &launch, |name| {
                (name == "LEAKED").then(|| "secret".into())
            })
        });
        daemon_end.write_all(b"ping\n").unwrap();
        let mut output = String::new();
        daemon_end.read_to_string(&mut output).unwrap();
        assert_eq!(output, "ping:value:unset");
        assert!(child.join().unwrap().unwrap().success());
    }

    #[test]
    fn refused_or_missing_daemon_starts_nothing() {
        let dir = std::env::temp_dir().join(format!("stackchan-plugin-run-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("plugin.sock");
        assert!(matches!(serve(&path), Ok(None)), "socket が無い");
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let refuse = thread::spawn(move || drop(listener.accept().unwrap()));
        assert!(matches!(serve(&path), Ok(None)), "起動情報の前に閉じられた");
        refuse.join().unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
