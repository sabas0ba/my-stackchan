//! port を占有し、複数の plugin からの表示要求を調停する常駐プロセス。
//!
//! 1 本のイベントループで plugin のメッセージ、再起動、slot の割当て、デバイスへの送信を
//! 扱う。非同期ランタイムは使わず、plugin の標準出力の読取りだけを plugin ごとの
//! スレッドで行う。設計は docs/plugin.md を参照する。

mod convert;
mod device;
mod plugin;
mod scheduler;

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use plugin_api::HostMessage;

use crate::PitchTrimFile;
use crate::config::Config;
use device::{Delivery, Device};
use plugin::{Event, Outcome, PluginRuntime, Request, Spawner};
use scheduler::{CardKey, Content, Plan, Scheduler, Shown, Source};

/// イベントが無い場合にも割当てと再起動を確認する間隔。
const TICK: Duration = Duration::from_millis(200);
/// 送り直しの間隔に加える firmware 側の TTL の余裕。daemon が停止した場合、
/// 表示は最長でこの時間だけ残る。
const DEVICE_TTL_MARGIN_S: u16 = 5;

pub fn run(config: Config, trim_file: PitchTrimFile) -> Result<(), Box<dyn std::error::Error>> {
    if config.plugins.is_empty() {
        return Err("設定に plugin がありません".into());
    }
    let device = device::SerialDevice::new(config.daemon.port.clone(), trim_file);
    let mut daemon = Daemon::new(config, device, plugin::OsSpawner, Instant::now());
    loop {
        daemon.step(TICK);
    }
}

const SLOTS: [protocol::Slot; 3] = [
    protocol::Slot::BannerTop,
    protocol::Slot::BannerBottom,
    protocol::Slot::Overlay,
];

#[derive(Debug, Clone, Copy)]
struct Sent {
    source: Source,
    at: Instant,
    /// firmware 側で表示が消える時刻。
    expires: Instant,
}

pub struct Daemon<D, S> {
    device: D,
    spawner: S,
    plugins: Vec<PluginRuntime>,
    scheduler: Scheduler,
    events: Receiver<Event>,
    sender: Sender<Event>,
    sent: [Option<Sent>; 3],
    visible: Vec<CardKey>,
    refresh: Duration,
    device_error: Option<String>,
}

impl<D: Device, S: Spawner> Daemon<D, S> {
    pub fn new(config: Config, device: D, spawner: S, now: Instant) -> Self {
        let (sender, events) = mpsc::channel();
        let refresh = Duration::from_secs(u64::from(config.daemon.rotate_s));
        Self {
            device,
            spawner,
            plugins: config
                .plugins
                .into_iter()
                .map(|plugin| PluginRuntime::new(plugin, now))
                .collect(),
            scheduler: Scheduler::new(refresh),
            events,
            sender,
            sent: [None; 3],
            visible: Vec::new(),
            refresh,
            device_error: None,
        }
    }

    /// イベントを最大 `timeout` 待って処理し、割当てを更新する。
    pub fn step(&mut self, timeout: Duration) {
        match self.events.recv_timeout(timeout) {
            Ok(event) => self.handle(event, Instant::now()),
            Err(RecvTimeoutError::Timeout) => {}
            // sender を自身が保持しているため切断されない。
            Err(RecvTimeoutError::Disconnected) => unreachable!(),
        }
        while let Ok(event) = self.events.try_recv() {
            self.handle(event, Instant::now());
        }
        self.tick(Instant::now());
    }

    fn handle(&mut self, event: Event, now: Instant) {
        match event {
            Event::Frame {
                plugin,
                generation,
                frame,
            } => {
                if !self.plugins[plugin].is_current(generation) {
                    return;
                }
                match self.plugins[plugin].receive(frame, now) {
                    Outcome::Nothing => {}
                    Outcome::Request(request) => self.apply(plugin, request, now),
                    Outcome::Reject(reason) => self.reject(plugin, reason),
                    Outcome::Stop(reason) => self.stop_plugin(plugin, &reason, now),
                }
            }
            Event::Closed { plugin, generation } => {
                if self.plugins[plugin].is_current(generation) {
                    self.stop_plugin(plugin, "終了しました", now);
                }
            }
        }
    }

    fn reject(&mut self, plugin: usize, reason: &str) {
        self.plugins[plugin].send(&HostMessage::Rejected {
            reason: reason.into(),
        });
    }

    fn stop_plugin(&mut self, plugin: usize, reason: &str, now: Instant) {
        self.plugins[plugin].stop(reason, now);
        self.scheduler.remove_plugin(plugin);
        self.visible.retain(|key| key.plugin != plugin);
    }

    fn apply(&mut self, plugin: usize, request: Request, now: Instant) {
        let result = match request {
            Request::Put(card) => convert::card_rows(&card).and_then(|rows| {
                self.scheduler.put(
                    CardKey {
                        plugin,
                        card: card.card,
                    },
                    card.placement,
                    card.priority,
                    card.ttl_s,
                    rows,
                    self.plugins[plugin].granted().cards,
                    now,
                )
            }),
            Request::Remove(card) => {
                self.scheduler.remove(CardKey { plugin, card });
                Ok(())
            }
            Request::Notify(notify) => convert::notice_text(&notify.text).map(|text| {
                self.scheduler
                    .notify(text.as_str().into(), notify.priority, notify.ttl_s)
            }),
            Request::Presence(presence) => {
                self.send_device(&protocol::Message::Presence(presence), now);
                Ok(())
            }
            Request::Emote(emote) => {
                self.send_device(&protocol::Message::Emote(emote), now);
                Ok(())
            }
        };
        if let Err(reason) = result {
            self.reject(plugin, reason);
        }
    }

    fn send_device(&mut self, message: &protocol::Message, now: Instant) -> bool {
        match self.device.send(message, now) {
            Ok(delivery) => {
                if self.device_error.take().is_some() {
                    eprintln!("[daemon] デバイスへの送信が回復しました");
                }
                if delivery == Delivery::Reset {
                    // 表示状態が失われたため、次の割当てで全 slot を送り直す。
                    self.sent = [None; 3];
                }
                true
            }
            Err(error) => {
                // 同じ誤りを割当ての周期ごとに繰り返し出力しない。
                if self.device_error.as_deref() != Some(error.as_str()) {
                    eprintln!("[daemon] デバイスへ送信できません: {error}");
                    self.device_error = Some(error);
                }
                false
            }
        }
    }

    fn tick(&mut self, now: Instant) {
        for event in self.device.poll() {
            self.on_device_event(event, now);
        }
        for index in 0..self.plugins.len() {
            self.plugins[index].start_if_due(index, now, &mut self.spawner, &self.sender);
            if self.plugins[index].check_timeout(now) {
                self.scheduler.remove_plugin(index);
            }
        }
        let plan = self.scheduler.plan(now);
        self.sync_slots(&plan, now);
        self.sync_visibility(&plan);
    }

    /// タップを振り分ける。行に action がある Card は発生元の plugin へ返し、帯のそれ以外の
    /// 位置は巡回を次へ送る。顔の領域と Overlay のそれ以外の位置では何もしない。
    fn on_device_event(&mut self, event: protocol::Event, now: Instant) {
        let protocol::Event::Tap { slot, card, action } = event;
        let key = card.and_then(CardKey::from_device_id);
        match (key, action) {
            (Some(key), Some(action))
                if key.plugin < self.plugins.len() && self.visible.contains(&key) =>
            {
                self.plugins[key.plugin].send(&HostMessage::Action {
                    card: key.card,
                    action,
                });
            }
            _ if matches!(
                slot,
                Some(protocol::Slot::BannerTop | protocol::Slot::BannerBottom)
            ) =>
            {
                self.scheduler.advance(now);
            }
            _ => {}
        }
    }

    fn sync_slots(&mut self, plan: &Plan, now: Instant) {
        let desired = [&plan.top, &plan.bottom, &plan.overlay];
        for (index, (slot, shown)) in SLOTS.into_iter().zip(desired).enumerate() {
            let previous = self.sent[index];
            match shown {
                Some(shown) => {
                    let unchanged = previous.is_some_and(|sent| {
                        sent.source == shown.source && now < sent.at + self.refresh
                    });
                    if unchanged {
                        continue;
                    }
                    let ttl_s = self.device_ttl(shown);
                    if self.send_device(&slot_message(slot, ttl_s, shown), now) {
                        self.sent[index] = Some(Sent {
                            source: shown.source,
                            at: now,
                            expires: now + Duration::from_secs(u64::from(ttl_s)),
                        });
                    }
                }
                None => {
                    let Some(sent) = previous else {
                        continue;
                    };
                    // 1 秒以内に firmware 側の期限で消える表示には、消去を送らない。
                    if sent.expires > now + Duration::from_secs(1)
                        && !self.send_device(&protocol::Message::ClearSlot(slot), now)
                    {
                        continue;
                    }
                    self.sent[index] = None;
                }
            }
        }
    }

    /// firmware 側の TTL。表示内容の残り時間と、送り直しの間隔 + 余裕の短い方とする。
    fn device_ttl(&self, shown: &Shown) -> u16 {
        let refresh = u16::try_from(self.refresh.as_secs())
            .unwrap_or(u16::MAX - DEVICE_TTL_MARGIN_S)
            .saturating_add(DEVICE_TTL_MARGIN_S);
        let remaining = shown.remaining.map_or(u16::MAX, |remaining| {
            // 切り上げ、期限より前に firmware 側で消えないようにする。
            let seconds = remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0);
            u16::try_from(seconds).unwrap_or(u16::MAX)
        });
        refresh.min(remaining).max(1)
    }

    fn sync_visibility(&mut self, plan: &Plan) {
        let visible: Vec<CardKey> = plan.visible_cards().collect();
        for key in &self.visible {
            if !visible.contains(key) {
                self.plugins[key.plugin].send(&HostMessage::Visibility {
                    card: key.card,
                    visible: false,
                });
            }
        }
        for key in &visible {
            if !self.visible.contains(key) {
                self.plugins[key.plugin].send(&HostMessage::Visibility {
                    card: key.card,
                    visible: true,
                });
            }
        }
        self.visible = visible;
    }
}

fn slot_message(slot: protocol::Slot, ttl_s: u16, shown: &Shown) -> protocol::Message {
    let id = match shown.source {
        Source::Card { key, .. } => key.device_id(),
        Source::Notice { .. } => 0,
    };
    match &shown.content {
        Content::Card(rows) => protocol::Message::Card(convert::device_card(slot, ttl_s, id, rows)),
        Content::Text(text) => protocol::Message::Text {
            slot,
            ttl_s,
            // Scheduler に入る前に長さを検証済みである。
            text: text.as_str().try_into().unwrap_or_default(),
        },
    }
}

#[cfg(test)]
mod tests;
