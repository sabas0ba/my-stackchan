//! daemon の結合試験。デバイスは送信内容を記録する fake、plugin は pipe で接続した
//! スレッドとし、plugin 側は plugin_api::client を用いる。

use std::io::{self, PipeReader, PipeWriter};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use plugin_api as api;
use plugin_api::client;

use super::device::{Delivery, Device};
use super::plugin::{Process, Spawned, Spawner};
use super::*;
use crate::config::PluginConfig;

#[derive(Clone, Default)]
struct FakeDevice {
    sent: Arc<Mutex<Vec<protocol::Message>>>,
    events: Arc<Mutex<Vec<protocol::Event>>>,
}

impl FakeDevice {
    fn messages(&self) -> Vec<protocol::Message> {
        self.sent.lock().unwrap().clone()
    }
}

impl Device for FakeDevice {
    fn send(&mut self, message: &protocol::Message, _now: Instant) -> Result<Delivery, String> {
        self.sent.lock().unwrap().push(message.clone());
        Ok(Delivery::Delivered)
    }

    fn poll(&mut self) -> Vec<protocol::Event> {
        std::mem::take(&mut *self.events.lock().unwrap())
    }
}

struct FakeProcess;

impl Process for FakeProcess {
    fn kill(&mut self) {}
}

/// plugin 側の pipe を試験スレッドへ渡す。
struct PipeSpawner {
    spawned: Arc<AtomicUsize>,
    plugin_ends: mpsc::Sender<(PipeReader, PipeWriter)>,
}

impl Spawner for PipeSpawner {
    fn spawn(&mut self, _config: &PluginConfig) -> io::Result<Spawned> {
        let (daemon_reader, plugin_writer) = io::pipe()?;
        let (plugin_reader, daemon_writer) = io::pipe()?;
        self.spawned.fetch_add(1, Ordering::SeqCst);
        self.plugin_ends
            .send((plugin_reader, plugin_writer))
            .map_err(|_| io::Error::other("試験側が終了しています"))?;
        Ok(Spawned {
            stdin: Box::new(daemon_writer),
            stdout: Box::new(daemon_reader),
            stderr: None,
            process: Box::new(FakeProcess),
        })
    }
}

struct Harness {
    daemon: Daemon<FakeDevice, PipeSpawner>,
    device: FakeDevice,
    spawned: Arc<AtomicUsize>,
    plugin_ends: mpsc::Receiver<(PipeReader, PipeWriter)>,
}

impl Harness {
    fn new(plugin_section: &str) -> Self {
        let config = crate::config::parse(
            &format!(
                "[daemon]\nrotate_s = 10\n[plugin test]\ncommand = [\"fake\"]\n{plugin_section}"
            ),
            None,
        )
        .unwrap();
        let device = FakeDevice::default();
        let spawned = Arc::new(AtomicUsize::new(0));
        let (sender, plugin_ends) = mpsc::channel();
        let spawner = PipeSpawner {
            spawned: spawned.clone(),
            plugin_ends: sender,
        };
        Self {
            daemon: Daemon::new(config, device.clone(), spawner, Instant::now()),
            device,
            spawned,
            plugin_ends,
        }
    }

    /// plugin を起動させ、plugin 側の処理を別スレッドで動かす。
    fn start_plugin<T: Send + 'static>(
        &mut self,
        body: impl FnOnce(PipeReader, PipeWriter) -> T + Send + 'static,
    ) -> thread::JoinHandle<T> {
        self.daemon.step(Duration::ZERO);
        let (reader, writer) = self
            .plugin_ends
            .recv_timeout(Duration::from_secs(5))
            .expect("plugin が起動されません");
        thread::spawn(move || body(reader, writer))
    }

    /// 条件を満たすまで daemon を進める。
    fn run_until(&mut self, what: &str, mut done: impl FnMut(&[protocol::Message]) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(&self.device.messages()) {
            assert!(Instant::now() < deadline, "{what} に至りません");
            self.daemon.step(Duration::from_millis(20));
        }
    }
}

fn hello(capabilities: api::Capabilities) -> api::Hello {
    api::Hello {
        api_version: api::API_VERSION,
        name: "test".into(),
        version: "0".into(),
        capabilities,
    }
}

fn banner(card: u8, text: &str) -> api::PluginMessage {
    api::PluginMessage::CardPut(api::CardPut {
        card,
        placement: api::Placement::Banner,
        priority: api::Priority::Normal,
        ttl_s: 0,
        rows: vec![api::Row {
            elements: vec![api::Element::Text { text: text.into() }],
            action: None,
        }],
    })
}

fn card_text(message: &protocol::Message) -> Option<(protocol::Slot, u16, String)> {
    let protocol::Message::Card(card) = message else {
        return None;
    };
    match &card.rows[0].elements[0] {
        protocol::Element::Text { text } => Some((card.slot, card.ttl_s, text.to_string())),
        _ => None,
    }
}

/// daemon からの `Rejected` か、接続の終了までを待つ。
fn next_rejection<W: io::Write>(connection: &mut client::Connection<W>) -> Option<String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !connection.is_closed() {
        if let Ok(Some(HostMessage::Rejected { reason })) =
            connection.recv_timeout(Duration::from_millis(50))
        {
            return Some(reason);
        }
    }
    None
}

#[test]
fn banner_card_reaches_device_with_refreshing_ttl() {
    let mut harness = Harness::new("cards = 1\nparam.utc_offset_minutes = \"540\"\n");
    let plugin = harness.start_plugin(|reader, writer| {
        let capabilities = api::Capabilities {
            cards: 1,
            ..api::Capabilities::default()
        };
        let mut connection = client::connect(reader, writer, hello(capabilities)).unwrap();
        let offset = connection.param("utc_offset_minutes").map(str::to_owned);
        connection.send(&banner(0, "12:34")).unwrap();
        // 表示されたことの通知を待つ。
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Ok(Some(HostMessage::Visibility {
                card: 0,
                visible: true,
            })) = connection.recv_timeout(Duration::from_millis(50))
            {
                return (offset, true);
            }
        }
        (offset, false)
    });
    harness.run_until("Card の送信", |sent| {
        sent.iter().any(|message| card_text(message).is_some())
    });
    let sent = harness.device.messages();
    let card = sent.iter().find_map(card_text).unwrap();
    // 送り直し間隔 10 秒に余裕 5 秒を加えた TTL で、daemon の停止後に表示が残らない。
    assert_eq!(card, (protocol::Slot::BannerTop, 15, "12:34".into()));
    // 下の帯には何も送らない。
    assert!(sent.iter().all(|message| !matches!(
        message,
        protocol::Message::Text {
            slot: protocol::Slot::BannerBottom,
            ..
        }
    )));
    for _ in 0..10 {
        harness.daemon.step(Duration::from_millis(20));
    }
    assert_eq!(
        plugin.join().unwrap(),
        (Some("540".to_owned()), true),
        "Init の param と Visibility が plugin に届く"
    );
}

#[test]
fn capabilities_are_enforced_per_message() {
    // presence は許可、motion と notify は許可しない。
    let mut harness = Harness::new("cards = 1\npresence = true\n");
    let plugin = harness.start_plugin(|reader, writer| {
        let requested = api::Capabilities {
            cards: 4,
            notify: true,
            presence: true,
            motion: true,
        };
        let mut connection = client::connect(reader, writer, hello(requested)).unwrap();
        let granted = connection.init().granted;
        connection
            .send(&api::PluginMessage::Emote(api::Emote {
                expression: api::Expression::Happy,
                gaze: api::Gaze::Center,
                eyes: api::EyeStyle::Auto,
                intensity: 80,
                duration_ms: 1000,
            }))
            .unwrap();
        connection
            .send(&api::PluginMessage::Notify(api::Notify {
                text: "hi".into(),
                priority: api::Priority::Normal,
                ttl_s: 0,
            }))
            .unwrap();
        let notify = next_rejection(&mut connection);
        connection.send(&banner(0, "a")).unwrap();
        connection.send(&banner(1, "b")).unwrap();
        let second_card = next_rejection(&mut connection);
        (granted, notify, second_card)
    });
    harness.run_until("Emote の送信", |sent| {
        sent.iter()
            .any(|message| matches!(message, protocol::Message::Emote(_)))
    });
    let (granted, notify, second_card) = loop {
        harness.daemon.step(Duration::from_millis(20));
        if plugin.is_finished() {
            break plugin.join().unwrap();
        }
    };
    assert_eq!(
        granted,
        api::Capabilities {
            cards: 1,
            notify: false,
            presence: true,
            motion: false
        }
    );
    assert!(notify.is_some(), "許可されていない通知は拒否される");
    assert!(second_card.is_some(), "許可数を超える Card は拒否される");
    let emote = harness
        .device
        .messages()
        .into_iter()
        .find_map(|message| match message {
            protocol::Message::Emote(emote) => Some(emote),
            _ => None,
        })
        .unwrap();
    assert_eq!(emote.intensity, 0, "motion が無い場合は機構部を動かさない");
}

#[test]
fn exited_plugin_is_cleared_and_restarted_with_backoff() {
    let mut harness = Harness::new("cards = 1\n");
    let plugin = harness.start_plugin(|reader, writer| {
        let capabilities = api::Capabilities {
            cards: 1,
            ..api::Capabilities::default()
        };
        let mut connection = client::connect(reader, writer, hello(capabilities)).unwrap();
        connection.send(&banner(0, "bye")).unwrap();
        // Card が表示されるまで待ってから終了する。
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline
            && !matches!(
                connection.recv_timeout(Duration::from_millis(50)),
                Ok(Some(HostMessage::Visibility { visible: true, .. }))
            )
        {}
    });
    harness.run_until("Card の送信", |sent| {
        sent.iter().any(|message| card_text(message).is_some())
    });
    plugin.join().unwrap();
    harness.run_until("帯の消去", |sent| {
        sent.iter().any(|message| {
            matches!(
                message,
                protocol::Message::ClearSlot(protocol::Slot::BannerTop)
            )
        })
    });
    assert_eq!(harness.spawned.load(Ordering::SeqCst), 1);
    // 1 回目の失敗後の待ち時間 (2 秒) を経て再起動する。
    let deadline = Instant::now() + Duration::from_secs(5);
    while harness.spawned.load(Ordering::SeqCst) < 2 {
        assert!(Instant::now() < deadline, "再起動されません");
        harness.daemon.step(Duration::from_millis(50));
    }
}

#[test]
fn protocol_violation_stops_the_plugin() {
    let mut harness = Harness::new("cards = 1\n");
    let plugin = harness.start_plugin(|reader, mut writer| {
        // Hello の代わりに別のメッセージを送る。
        api::write_frame(&mut writer, &banner(0, "x")).unwrap();
        let mut frames = api::FrameReader::new(reader);
        // daemon が停止すると stdin が閉じ、読取りが終わる。
        frames.read_frame::<HostMessage>().ok().flatten()
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while !plugin.is_finished() {
        assert!(Instant::now() < deadline, "plugin が停止されません");
        harness.daemon.step(Duration::from_millis(20));
    }
    assert_eq!(plugin.join().unwrap(), None, "Init は送られない");
    assert!(
        harness
            .device
            .messages()
            .iter()
            .all(|message| card_text(message).is_none())
    );
}

#[test]
fn tap_on_card_row_is_returned_to_the_plugin_as_action() {
    let mut harness = Harness::new(
        "cards = 1
",
    );
    let plugin = harness.start_plugin(|reader, writer| {
        let capabilities = api::Capabilities {
            cards: 1,
            ..api::Capabilities::default()
        };
        let mut connection = client::connect(reader, writer, hello(capabilities)).unwrap();
        let api::PluginMessage::CardPut(mut card) = banner(3, "tap me") else {
            unreachable!()
        };
        card.rows[0].action = Some(5);
        connection.send(&api::PluginMessage::CardPut(card)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Ok(Some(HostMessage::Action { card, action })) =
                connection.recv_timeout(Duration::from_millis(50))
            {
                return Some((card, action));
            }
        }
        None
    });
    harness.run_until("Card の送信", |sent| {
        sent.iter().any(|message| card_text(message).is_some())
    });
    let sent = harness.device.messages();
    let card = sent
        .iter()
        .find_map(|message| match message {
            protocol::Message::Card(card) => Some(card.clone()),
            _ => None,
        })
        .unwrap();
    assert_ne!(card.id, 0, "daemon の Card には識別子が付く");
    assert_eq!(card.rows[0].action, Some(5));
    harness
        .device
        .events
        .lock()
        .unwrap()
        .push(protocol::Event::Tap {
            slot: Some(protocol::Slot::BannerTop),
            card: Some(card.id),
            action: Some(5),
        });
    let deadline = Instant::now() + Duration::from_secs(10);
    while !plugin.is_finished() {
        assert!(Instant::now() < deadline, "Action が plugin に届きません");
        harness.daemon.step(Duration::from_millis(20));
    }
    assert_eq!(plugin.join().unwrap(), Some((3, 5)));
}

/// 書込みが戻らない stdin。plugin が stdin を読まない状況を再現する。
struct StalledStdin;

impl io::Write for StalledStdin {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        loop {
            thread::park();
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct KillFlag(Arc<std::sync::atomic::AtomicBool>);

impl Process for KillFlag {
    fn kill(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// stdin を読まない plugin を 1 つだけ起動する。
struct StalledSpawner {
    plugin_stdout: Option<PipeReader>,
    killed: Arc<std::sync::atomic::AtomicBool>,
}

impl Spawner for StalledSpawner {
    fn spawn(&mut self, _config: &PluginConfig) -> io::Result<Spawned> {
        let stdout = self
            .plugin_stdout
            .take()
            .ok_or_else(|| io::Error::other("再起動は試験の対象外"))?;
        Ok(Spawned {
            stdin: Box::new(StalledStdin),
            stdout: Box::new(stdout),
            stderr: None,
            process: Box::new(KillFlag(self.killed.clone())),
        })
    }
}

#[test]
fn plugin_that_does_not_read_stdin_is_stopped_without_blocking_the_loop() {
    let config =
        crate::config::parse("[plugin stalled]\ncommand = [\"fake\"]\ncards = 1\n", None).unwrap();
    let (daemon_reader, mut plugin_writer) = io::pipe().unwrap();
    let killed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let spawner = StalledSpawner {
        plugin_stdout: Some(daemon_reader),
        killed: killed.clone(),
    };
    let mut daemon = Daemon::new(config, FakeDevice::default(), spawner, Instant::now());
    daemon.step(Duration::ZERO);

    // 拒否されるメッセージを流量の上限未満で送り続け、Rejected を溜めさせる。
    let capabilities = api::Capabilities {
        cards: 1,
        ..api::Capabilities::default()
    };
    api::write_frame(
        &mut plugin_writer,
        &api::PluginMessage::Hello(hello(capabilities)),
    )
    .unwrap();
    for _ in 0..40 {
        api::write_frame(
            &mut plugin_writer,
            &api::PluginMessage::CardRemove {
                card: api::MAX_CARDS,
            },
        )
        .unwrap();
    }

    let deadline = Instant::now() + Duration::from_secs(10);
    while !killed.load(Ordering::SeqCst) {
        assert!(Instant::now() < deadline, "plugin が停止されません");
        let started = Instant::now();
        daemon.step(Duration::from_millis(20));
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "イベントループが plugin への書込みで止まっています"
        );
    }
}

#[test]
fn spool_requests_reach_the_device_and_are_removed() {
    let dir = std::env::temp_dir().join(format!("stackchan-daemon-spool-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for request in [
        crate::spool::Request::Notify {
            text: "hook".into(),
            priority: api::Priority::Normal,
            ttl_s: 5,
        },
        crate::spool::Request::Status {
            activity: protocol::Activity::Waiting,
            detail: "INPUT".into(),
            ttl_s: 30,
        },
        crate::spool::Request::Input(protocol::InputMode::Forward),
    ] {
        crate::spool::write(&dir, &request).unwrap();
    }
    let config = crate::config::parse("[plugin idle]\ncommand = [\"fake\"]\n", None).unwrap();
    let (sender, _plugin_ends) = mpsc::channel();
    let spawner = PipeSpawner {
        spawned: Arc::new(AtomicUsize::new(0)),
        plugin_ends: sender,
    };
    let device = FakeDevice::default();
    let mut daemon =
        Daemon::new(config, device.clone(), spawner, Instant::now()).with_spool(dir.clone());
    daemon.step(Duration::ZERO);

    let sent = device.messages();
    assert!(sent.iter().any(|message| matches!(
        message,
        protocol::Message::Text { slot: protocol::Slot::BannerBottom, text, .. } if text == "hook"
    )));
    assert!(sent.iter().any(|message| matches!(
        message,
        protocol::Message::Presence(presence)
            if presence.activity == Some(protocol::Activity::Waiting)
                && presence.expression == protocol::Expression::Sleepy
                && presence.detail == "INPUT"
    )));
    assert!(sent.contains(&protocol::Message::InputMode(protocol::InputMode::Forward)));
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        0,
        "処理した要求は削除する"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}
