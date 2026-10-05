//! plugin からの接続を Unix socket で待ち受ける起動方式 (plugin ごとのコンテナ分離)。
//!
//! daemon は plugin を起動しない。plugin のコンテナで動く `plugin-run` が
//! `<socket dir>/<plugin id>/plugin.sock` へ接続し、daemon は起動情報を送ってから、
//! 接続を plugin の stdin / stdout として扱う。plugin ごとのディレクトリは当該 plugin の
//! コンテナにだけマウントするため、接続を受けた socket で plugin を識別できる。
//! 設計は docs/plugin.md の「plugin ごとのコンテナ分離」を参照する。

use std::collections::HashMap;
use std::io;
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::time::Duration;

use plugin_api::Endpoint;

use super::plugin::{Process, Spawned, Spawner};
use crate::config::{Config, PluginConfig};
use crate::launch;

pub const SOCKET_NAME: &str = "plugin.sock";
/// 起動情報の書込みを待つ時間。イベントループで書くため、読まない相手で止まらないようにする。
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(1);

struct Peer(UnixStream);

impl Process for Peer {
    /// 接続を閉じる。別のコンテナにある plugin のプロセスを終了させる手段は持たない。
    fn kill(&mut self) {
        let _ = self.0.shutdown(Shutdown::Both);
    }
}

pub struct SocketSpawner {
    listeners: HashMap<String, UnixListener>,
}

impl SocketSpawner {
    pub fn new(dir: &Path, config: &Config) -> io::Result<Self> {
        let mut listeners = HashMap::new();
        for plugin in &config.plugins {
            let plugin_dir = dir.join(&plugin.id);
            std::fs::create_dir_all(&plugin_dir)?;
            let path = plugin_dir.join(SOCKET_NAME);
            // 前回の daemon が残した socket があると bind できない。
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            let listener = UnixListener::bind(&path)?;
            listener.set_nonblocking(true)?;
            listeners.insert(plugin.id.clone(), listener);
        }
        Ok(Self { listeners })
    }

    /// 待っている接続を 1 本取り出す。無ければ None。
    fn accept(&self, id: &str) -> io::Result<Option<UnixStream>> {
        let listener = self
            .listeners
            .get(id)
            .ok_or_else(|| io::Error::other("待受けの無い plugin です"))?;
        match listener.accept() {
            Ok((stream, _)) => Ok(Some(stream)),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// 待っている接続をすべて閉じる。
    fn drain(&self, id: &str) {
        while let Ok(Some(stream)) = self.accept(id) {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}

impl Spawner for SocketSpawner {
    fn spawn(&mut self, config: &PluginConfig) -> io::Result<Option<Spawned>> {
        let Some(mut stream) = self.accept(&config.id)? else {
            return Ok(None);
        };
        // 同じ plugin の接続は 1 本だけを保持する。
        self.drain(&config.id);
        stream.set_nonblocking(false)?;
        stream.set_write_timeout(Some(LAUNCH_TIMEOUT))?;
        launch::send(&mut stream, config).map_err(io::Error::other)?;
        stream.set_write_timeout(None)?;
        Ok(Some(Spawned {
            stdin: Box::new(stream.try_clone()?),
            stdout: Box::new(stream.try_clone()?),
            stderr: None,
            process: Box::new(Peer(stream)),
        }))
    }

    fn refuse(&mut self, config: &PluginConfig) {
        self.drain(&config.id);
    }

    /// 中継の socket を、plugin のコンテナから見た path で知らせる。
    fn endpoints(&self, config: &PluginConfig) -> Vec<Endpoint> {
        config
            .net_allow
            .iter()
            .enumerate()
            .map(|(index, addr)| Endpoint {
                addr: addr.to_string(),
                path: format!("{}/{}", launch::PLUGIN_MOUNT, launch::relay_socket(index)),
            })
            .collect()
    }
}
