//! plugin プロセスの起動・監視と、plugin からのメッセージの検査。
//!
//! plugin は信頼しない。手順違反 (Hello 以外で始まる、復号できないフレーム、流量超過) は
//! plugin を停止し、指数的な待ち時間の後に再起動する。内容の誤り (検証失敗、許可外の
//! capability) はメッセージ単位で拒否し、plugin へ理由を返す。

use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Sender, SyncSender, TrySendError};
use std::thread;
use std::time::{Duration, Instant};

use plugin_api::{
    API_VERSION, Capabilities, CardPut, FrameError, FrameReader, HostMessage, Init, PluginMessage,
};

use super::convert;
use crate::config::PluginConfig;
use crate::log;

/// Hello を待つ時間。
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
/// 1 秒あたりに受け付けるメッセージ数。超えた plugin は停止する。
const MAX_MESSAGES_PER_SECOND: u32 = 50;
/// 表情の要求の最短間隔。Emote は上書きされるため、連続送信で顔が落ち着かなくなるのを防ぐ。
const MIN_FACE_INTERVAL: Duration = Duration::from_secs(1);
/// 連続して失敗した場合に無効化するまでの回数。
const MAX_FAILURES: u32 = 5;
/// この時間以上動作した plugin の停止は連続失敗に数えない。
const STABLE_RUN: Duration = Duration::from_secs(60);
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// stderr の 1 行としてログへ出す最大バイト数。
const MAX_STDERR_LINE: usize = 1024;
/// plugin へ送るメッセージの待ち行列の長さ。溢れた plugin は stdin を読んでいないとみなし、
/// 停止する。書込みをイベントループで待つと、読まない plugin 1 つで daemon 全体が止まる。
const OUTGOING_QUEUE: usize = 32;

pub trait Process: Send {
    fn kill(&mut self);
}

impl Process for Child {
    fn kill(&mut self) {
        let _ = Child::kill(self);
        let _ = self.wait();
    }
}

pub struct Spawned {
    pub stdin: Box<dyn Write + Send>,
    pub stdout: Box<dyn Read + Send>,
    pub stderr: Option<Box<dyn Read + Send>>,
    pub process: Box<dyn Process>,
}

/// plugin の起動方法。試験ではプロセスの代わりに pipe を返す。
pub trait Spawner {
    fn spawn(&mut self, config: &PluginConfig) -> io::Result<Spawned>;
}

/// plugin に引き継ぐ daemon の環境変数。daemon の環境 (利用者のシェルにあるトークン等) を
/// そのまま渡さないため、実行に必要なものだけに限る。Windows の変数名は大文字小文字を区別しない。
const INHERITED_ENV: [&str; 10] = [
    "PATH",
    "HOME",
    "USERPROFILE",
    "LANG",
    "LC_ALL",
    "TZ",
    "TMPDIR",
    "TEMP",
    "TMP",
    "SYSTEMROOT",
];

/// plugin の環境変数。許可した daemon の変数と、利用者設定の `env.*` だけからなる。
fn plugin_env(
    config: &PluginConfig,
    lookup: impl Fn(&str) -> Option<std::ffi::OsString>,
) -> Vec<(String, std::ffi::OsString)> {
    let mut env: Vec<(String, std::ffi::OsString)> = INHERITED_ENV
        .iter()
        .filter_map(|name| lookup(name).map(|value| ((*name).to_owned(), value)))
        .collect();
    for (name, value) in &config.env {
        env.retain(|(existing, _)| existing != name);
        env.push((name.clone(), value.into()));
    }
    env
}

pub struct OsSpawner;

impl Spawner for OsSpawner {
    fn spawn(&mut self, config: &PluginConfig) -> io::Result<Spawned> {
        let mut child = Command::new(&config.command[0])
            .args(&config.command[1..])
            .env_clear()
            .envs(plugin_env(config, |name| std::env::var_os(name)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let missing = || io::Error::other("子プロセスの標準入出力を取得できません");
        let stdin = child.stdin.take().ok_or_else(missing)?;
        let stdout = child.stdout.take().ok_or_else(missing)?;
        let stderr = child.stderr.take().ok_or_else(missing)?;
        Ok(Spawned {
            stdin: Box::new(stdin),
            stdout: Box::new(stdout),
            stderr: Some(Box::new(stderr)),
            process: Box::new(child),
        })
    }
}

pub enum Event {
    Frame {
        plugin: usize,
        generation: u64,
        frame: Result<PluginMessage, FrameError>,
    },
    Closed {
        plugin: usize,
        generation: u64,
    },
}

/// plugin の要求のうち、daemon が画面または顔に反映するもの。
pub enum Request {
    Put(CardPut),
    Remove(u8),
    Notify(plugin_api::Notify),
    Presence(protocol::Presence),
    Emote(protocol::Emote),
}

pub enum Outcome {
    Nothing,
    Request(Request),
    /// メッセージ単位の拒否。plugin へ理由を返し、動作は続ける。
    Reject(&'static str),
    /// 手順違反。plugin を停止する。
    Stop(String),
}

struct Io {
    /// plugin ごとの書込みスレッドへの待ち行列。破棄すると書込みスレッドが終わり、
    /// plugin の stdin が閉じる。
    outgoing: SyncSender<HostMessage>,
    process: Box<dyn Process>,
}

enum State {
    Idle {
        retry_at: Instant,
    },
    Handshake {
        io: Io,
        deadline: Instant,
        started: Instant,
    },
    Running {
        io: Io,
        granted: Capabilities,
        started: Instant,
    },
    Disabled,
}

pub struct PluginRuntime {
    pub config: PluginConfig,
    generation: u64,
    state: State,
    failures: u32,
    window: (Instant, u32),
    last_face: Option<Instant>,
}

impl PluginRuntime {
    pub fn new(config: PluginConfig, now: Instant) -> Self {
        Self {
            config,
            generation: 0,
            state: State::Idle { retry_at: now },
            failures: 0,
            window: (now, 0),
            last_face: None,
        }
    }

    pub fn is_current(&self, generation: u64) -> bool {
        self.generation == generation
    }

    pub fn granted(&self) -> Capabilities {
        match &self.state {
            State::Running { granted, .. } => *granted,
            _ => Capabilities::default(),
        }
    }

    /// 再起動の時刻に達していれば起動する。
    pub fn start_if_due(
        &mut self,
        index: usize,
        now: Instant,
        spawner: &mut impl Spawner,
        events: &Sender<Event>,
    ) {
        let State::Idle { retry_at } = self.state else {
            return;
        };
        if now < retry_at {
            return;
        }
        let spawned = match spawner.spawn(&self.config) {
            Ok(spawned) => spawned,
            Err(error) => {
                log::error!(
                    "daemon",
                    "plugin {} を起動できません: {error}",
                    self.config.id
                );
                self.fail(now, now);
                return;
            }
        };
        self.generation += 1;
        let generation = self.generation;
        let sender = events.clone();
        thread::spawn(move || {
            let mut frames = FrameReader::new(spawned.stdout);
            loop {
                let frame = match frames.read_frame::<PluginMessage>() {
                    Ok(Some(message)) => Ok(message),
                    Ok(None) => break,
                    Err(error) => Err(error),
                };
                let fatal = matches!(frame, Err(FrameError::Io(_)));
                let event = Event::Frame {
                    plugin: index,
                    generation,
                    frame,
                };
                if sender.send(event).is_err() || fatal {
                    break;
                }
            }
            let _ = sender.send(Event::Closed {
                plugin: index,
                generation,
            });
        });
        let (outgoing, queued) = mpsc::sync_channel::<HostMessage>(OUTGOING_QUEUE);
        let mut stdin = spawned.stdin;
        thread::spawn(move || {
            for message in queued {
                // 書込みの失敗は plugin の終了によるものであり、Closed の通知で扱う。
                if plugin_api::write_frame(&mut stdin, &message).is_err() {
                    break;
                }
            }
        });
        if let Some(stderr) = spawned.stderr {
            let id = self.config.id.clone();
            thread::spawn(move || forward_stderr(&id, stderr));
        }
        let rev = self.config.rev.as_deref().unwrap_or("-");
        log::info!(
            "daemon",
            "plugin {} を起動しました (rev {rev})",
            self.config.id
        );
        self.window = (now, 0);
        self.state = State::Handshake {
            io: Io {
                outgoing,
                process: spawned.process,
            },
            deadline: now + HANDSHAKE_TIMEOUT,
            started: now,
        };
    }

    /// Hello を期限内に送らない plugin を停止する。停止した場合は真を返す。
    pub fn check_timeout(&mut self, now: Instant) -> bool {
        if let State::Handshake { deadline, .. } = self.state
            && now >= deadline
        {
            self.stop("Hello を受信できませんでした", now);
            return true;
        }
        false
    }

    /// plugin へ送る。待ち行列が溢れた場合は理由を返し、呼出し側が plugin を停止する。
    pub fn send(&mut self, message: &HostMessage) -> Result<(), &'static str> {
        let (State::Handshake { io, .. } | State::Running { io, .. }) = &mut self.state else {
            return Ok(());
        };
        match io.outgoing.try_send(message.clone()) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err("plugin が標準入力を読んでいません"),
            // 書込みスレッドの終了は plugin の終了によるものであり、Closed の通知で扱う。
            Err(TrySendError::Disconnected(_)) => Ok(()),
        }
    }

    /// plugin を停止し、再起動を予約する。
    pub fn stop(&mut self, reason: &str, now: Instant) {
        let started = match std::mem::replace(&mut self.state, State::Disabled) {
            State::Handshake {
                mut io, started, ..
            }
            | State::Running {
                mut io, started, ..
            } => {
                io.process.kill();
                started
            }
            other => {
                self.state = other;
                return;
            }
        };
        // 旧プロセスから遅れて届くイベントを無視するため、世代を進める。
        self.generation += 1;
        log::warning!(
            "daemon",
            "plugin {} を停止しました: {reason}",
            self.config.id
        );
        self.fail(started, now);
    }

    fn fail(&mut self, started: Instant, now: Instant) {
        if now.duration_since(started) >= STABLE_RUN {
            self.failures = 0;
        }
        self.failures += 1;
        if self.failures > MAX_FAILURES {
            log::error!(
                "daemon",
                "plugin {} は {MAX_FAILURES} 回続けて失敗したため無効にしました",
                self.config.id
            );
            self.state = State::Disabled;
            return;
        }
        let backoff = Duration::from_secs(1 << self.failures.min(6)).min(MAX_BACKOFF);
        self.state = State::Idle {
            retry_at: now + backoff,
        };
    }

    /// plugin からのフレームを検査し、daemon が行う処理を返す。
    pub fn receive(&mut self, frame: Result<PluginMessage, FrameError>, now: Instant) -> Outcome {
        let message = match frame {
            Ok(message) => message,
            Err(error) => return Outcome::Stop(format!("不正なフレーム: {error}")),
        };
        if now.duration_since(self.window.0) >= Duration::from_secs(1) {
            self.window = (now, 0);
        }
        self.window.1 += 1;
        if self.window.1 > MAX_MESSAGES_PER_SECOND {
            return Outcome::Stop("メッセージの流量が上限を超えました".into());
        }

        if let State::Handshake { .. } = self.state {
            return self.handshake(message);
        }
        let State::Running { granted, .. } = self.state else {
            return Outcome::Nothing;
        };
        if let Err(reason) = message.validate() {
            return Outcome::Reject(reason);
        }
        match message {
            PluginMessage::Hello(_) => Outcome::Stop("Hello を再送しました".into()),
            PluginMessage::CardPut(card) if granted.cards > 0 => {
                Outcome::Request(Request::Put(card))
            }
            PluginMessage::CardRemove { card } => Outcome::Request(Request::Remove(card)),
            PluginMessage::Notify(notify) if granted.notify => {
                Outcome::Request(Request::Notify(notify))
            }
            PluginMessage::Presence(presence) if granted.presence => {
                self.face_request(now, || convert::presence(&presence).map(Request::Presence))
            }
            PluginMessage::Emote(emote) if granted.presence => self.face_request(now, || {
                convert::emote(&emote, granted.motion).map(Request::Emote)
            }),
            PluginMessage::Log { level, text } => {
                let level = match level {
                    plugin_api::LogLevel::Error => log::Level::Error,
                    plugin_api::LogLevel::Warn => log::Level::Warn,
                    plugin_api::LogLevel::Info => log::Level::Info,
                    plugin_api::LogLevel::Debug => log::Level::Debug,
                };
                let source = format!("plugin {}", self.config.id);
                log::write(level, &source, format_args!("{}", text.escape_debug()));
                Outcome::Nothing
            }
            PluginMessage::CardPut(_)
            | PluginMessage::Notify(_)
            | PluginMessage::Presence(_)
            | PluginMessage::Emote(_) => Outcome::Reject("許可されていない操作です"),
        }
    }

    fn face_request(
        &mut self,
        now: Instant,
        convert: impl FnOnce() -> Result<Request, &'static str>,
    ) -> Outcome {
        if self
            .last_face
            .is_some_and(|last| now.duration_since(last) < MIN_FACE_INTERVAL)
        {
            return Outcome::Reject("表情の要求の間隔が短すぎます");
        }
        match convert() {
            Ok(request) => {
                self.last_face = Some(now);
                Outcome::Request(request)
            }
            Err(reason) => Outcome::Reject(reason),
        }
    }

    fn handshake(&mut self, message: PluginMessage) -> Outcome {
        let PluginMessage::Hello(hello) = &message else {
            return Outcome::Stop("最初のメッセージが Hello ではありません".into());
        };
        if let Err(reason) = message.validate() {
            return Outcome::Stop(format!("Hello が不正です: {reason}"));
        }
        if hello.api_version != API_VERSION {
            return Outcome::Stop(format!(
                "plugin API の版が一致しません: daemon={API_VERSION}, plugin={}",
                hello.api_version
            ));
        }
        let granted = self.config.allowed.intersect(hello.capabilities);
        log::info!(
            "daemon",
            "plugin {}: {} {} (要求 {:?}, 許可 {granted:?})",
            self.config.id,
            hello.name.escape_debug(),
            hello.version.escape_debug(),
            hello.capabilities
        );
        let State::Handshake { io, started, .. } =
            std::mem::replace(&mut self.state, State::Disabled)
        else {
            unreachable!("handshake は Handshake 状態でのみ呼ばれる");
        };
        self.state = State::Running {
            io,
            granted,
            started,
        };
        let init = HostMessage::Init(Init {
            api_version: API_VERSION,
            granted,
            params: self.config.params.clone(),
            limits: convert::LIMITS,
        });
        if log::enabled(log::Level::Trace) {
            let source = format!("plugin {}", self.config.id);
            // Init の Debug は params の値を伏せる (plugin-api)。
            log::trace!(source, "送信 {init:?}");
        }
        match self.send(&init) {
            Ok(()) => Outcome::Nothing,
            Err(reason) => Outcome::Stop(reason.into()),
        }
    }
}

/// stderr を行単位でログへ転送する。1 行は上限で切り詰め、確保量を抑える。
fn forward_stderr(id: &str, stderr: Box<dyn Read + Send>) {
    let mut reader = BufReader::new(stderr);
    let mut line = Vec::new();
    loop {
        let available = match reader.fill_buf() {
            Ok([]) | Err(_) => break,
            Ok(bytes) => bytes,
        };
        let (chunk, newline) = match available.iter().position(|&b| b == b'\n') {
            Some(index) => (&available[..index], true),
            None => (available, false),
        };
        let room = MAX_STDERR_LINE.saturating_sub(line.len());
        line.extend_from_slice(&chunk[..chunk.len().min(room)]);
        let consumed = chunk.len() + usize::from(newline);
        reader.consume(consumed);
        if newline {
            // 制御文字 (端末の色指定、復帰) で daemon のログの表示を偽装されないようにする。
            log::info!(
                format!("plugin {id} stderr"),
                "{}",
                String::from_utf8_lossy(&line).trim_end().escape_debug()
            );
            line.clear();
        }
    }
    if !line.is_empty() {
        log::info!(
            format!("plugin {id} stderr"),
            "{}",
            String::from_utf8_lossy(&line).trim_end().escape_debug()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugins_receive_only_allowed_and_configured_environment() {
        let config = crate::config::parse(
            "[plugin p]\ncommand = [\"x\"]\nenv.HTTP_PROXY = \"http://proxy:8080\"\nenv.PATH = \"/opt/bin\"\n",
            None,
        )
        .unwrap();
        let daemon_env = |name: &str| match name {
            "PATH" => Some("/usr/bin".into()),
            "HOME" => Some("/root".into()),
            "GH_TOKEN" | "ANTHROPIC_API_KEY" => Some("secret".into()),
            _ => None,
        };
        let env = plugin_env(&config.plugins[0], daemon_env);
        let names: Vec<&str> = env.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["HOME", "HTTP_PROXY", "PATH"]);
        assert!(
            env.contains(&("PATH".into(), "/opt/bin".into())),
            "設定が優先する"
        );
        assert!(env.iter().all(|(_, value)| value != "secret"));
    }
}
