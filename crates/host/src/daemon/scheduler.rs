//! plugin の Card と通知を firmware の slot へ割り当てる。
//!
//! 時刻は引数で受け取り、入出力を持たない。巡回・割込み・期限の規則を単体で試験する
//! ためである。規則は docs/plugin.md の「画面と入力の配分」に従う。
//!
//! - 帯: 常設の Card を優先度順に並べ、2 枚ずつ上下の帯に置いて一定間隔で巡回する。
//!   高優先度以外の通知がある間は、下の帯を通知に、上の帯を Card 1 枚ずつの巡回に使う
//! - Overlay: 一時的な Card (TTL 必須) と高優先度の通知のうち、優先度が高く新しいもの

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use plugin_api::{Placement, Priority};

use super::convert::Rows;

/// 通知の TTL が 0 の場合の表示時間。
const DEFAULT_NOTICE: Duration = Duration::from_secs(10);
/// 待ち行列に置ける通知の数。超えた分は古いものから捨てる。
const MAX_PENDING_NOTICES: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CardKey {
    pub plugin: usize,
    pub card: u8,
}

impl CardKey {
    /// デバイスの Card 識別子。0 は識別なしに予約されているため 1 から始める。
    /// plugin は設定の上限 (16) まで、Card は plugin API の上限 (8) までで u16 に収まる。
    pub fn device_id(self) -> u16 {
        let index = self.plugin * usize::from(plugin_api::MAX_CARDS) + usize::from(self.card);
        u16::try_from(index + 1).expect("plugin 数と Card 数の上限で u16 に収まる")
    }

    pub fn from_device_id(id: u16) -> Option<Self> {
        let index = usize::from(id.checked_sub(1)?);
        let cards = usize::from(plugin_api::MAX_CARDS);
        Some(Self {
            plugin: index / cards,
            card: (index % cards) as u8,
        })
    }
}

/// 表示内容の出所と版。同じ値なら同じ内容であり、送り直す必要が無い。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Card { key: CardKey, revision: u64 },
    Notice { id: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    /// 行は 500 byte を超えるため、Text との大きさの差を Box で抑える。
    Card(Box<Rows>),
    Text(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    pub source: Source,
    pub content: Content,
    /// 期限までの残り。None は期限なし。
    pub remaining: Option<Duration>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    pub top: Option<Shown>,
    pub bottom: Option<Shown>,
    pub overlay: Option<Shown>,
}

impl Plan {
    /// 表示中の Card。Visibility の通知に使う。
    pub fn visible_cards(&self) -> impl Iterator<Item = CardKey> + '_ {
        [&self.top, &self.bottom, &self.overlay]
            .into_iter()
            .flatten()
            .filter_map(|shown| match shown.source {
                Source::Card { key, .. } => Some(key),
                Source::Notice { .. } => None,
            })
    }
}

#[derive(Debug, Clone)]
struct CardEntry {
    key: CardKey,
    placement: Placement,
    priority: Priority,
    rows: Rows,
    deadline: Option<Instant>,
    revision: u64,
}

#[derive(Debug, Clone)]
struct Notice {
    id: u64,
    text: String,
    priority: Priority,
    duration: Duration,
}

#[derive(Debug)]
pub struct Scheduler {
    rotate: Duration,
    cards: Vec<CardEntry>,
    pending: VecDeque<Notice>,
    current: Option<(Notice, Instant)>,
    counter: u64,
    page: usize,
    next_rotation: Option<Instant>,
}

impl Scheduler {
    pub fn new(rotate: Duration) -> Self {
        Self {
            rotate,
            cards: Vec::new(),
            pending: VecDeque::new(),
            current: None,
            counter: 0,
            page: 0,
            next_rotation: None,
        }
    }

    fn next_id(&mut self) -> u64 {
        self.counter += 1;
        self.counter
    }

    /// Card を追加または置き換える。plugin が同時に持つ Card は `limit` 枚までとする。
    #[allow(clippy::too_many_arguments)]
    pub fn put(
        &mut self,
        key: CardKey,
        placement: Placement,
        priority: Priority,
        ttl_s: u16,
        rows: Rows,
        limit: u8,
        now: Instant,
    ) -> Result<(), &'static str> {
        self.expire(now);
        if placement == Placement::Overlay && ttl_s == 0 {
            // 期限の無い Overlay は顔を隠し続けるため受け付けない。
            return Err("Overlay の Card には TTL を指定してください");
        }
        let held = self
            .cards
            .iter()
            .filter(|entry| entry.key.plugin == key.plugin && entry.key != key)
            .count();
        if held >= usize::from(limit) {
            return Err("許可された Card の数を超えます");
        }
        let revision = self.next_id();
        let entry = CardEntry {
            key,
            placement,
            priority,
            rows,
            deadline: (ttl_s != 0).then(|| now + Duration::from_secs(u64::from(ttl_s))),
            revision,
        };
        match self.cards.iter_mut().find(|existing| existing.key == key) {
            Some(existing) => *existing = entry,
            None => self.cards.push(entry),
        }
        Ok(())
    }

    pub fn remove(&mut self, key: CardKey) {
        self.cards.retain(|entry| entry.key != key);
    }

    /// plugin の停止時に、その plugin の Card をすべて取り除く。
    pub fn remove_plugin(&mut self, plugin: usize) {
        self.cards.retain(|entry| entry.key.plugin != plugin);
    }

    pub fn notify(&mut self, text: String, priority: Priority, ttl_s: u16) {
        let duration = match ttl_s {
            0 => DEFAULT_NOTICE,
            seconds => Duration::from_secs(u64::from(seconds)),
        };
        let id = self.next_id();
        if self.pending.len() == MAX_PENDING_NOTICES {
            self.pending.pop_front();
        }
        self.pending.push_back(Notice {
            id,
            text,
            priority,
            duration,
        });
    }

    fn expire(&mut self, now: Instant) {
        self.cards
            .retain(|entry| entry.deadline.is_none_or(|deadline| now < deadline));
        if self
            .current
            .as_ref()
            .is_some_and(|(_, deadline)| now >= *deadline)
        {
            self.current = None;
        }
        if self.current.is_none()
            && let Some(notice) = self.pending.pop_front()
        {
            let deadline = now + notice.duration;
            self.current = Some((notice, deadline));
        }
    }

    fn shown_card(entry: &CardEntry, now: Instant) -> Shown {
        Shown {
            source: Source::Card {
                key: entry.key,
                revision: entry.revision,
            },
            content: Content::Card(Box::new(entry.rows.clone())),
            remaining: entry.deadline.map(|deadline| deadline - now),
        }
    }

    /// 帯の巡回を次へ送る。利用者が帯の空いた所をタップした場合に使う。
    pub fn advance(&mut self, now: Instant) {
        self.page = self.page.wrapping_add(1);
        self.next_rotation = Some(now + self.rotate);
    }

    /// 現在の割当てを求める。巡回の位置もここで進める。
    pub fn plan(&mut self, now: Instant) -> Plan {
        self.expire(now);
        match self.next_rotation {
            Some(at) if now >= at => {
                self.page = self.page.wrapping_add(1);
                self.next_rotation = Some(now + self.rotate);
            }
            None => self.next_rotation = Some(now + self.rotate),
            Some(_) => {}
        }

        let notice = self.current.as_ref().map(|(notice, deadline)| {
            (
                notice.priority,
                Shown {
                    source: Source::Notice { id: notice.id },
                    content: Content::Text(notice.text.clone()),
                    remaining: Some(*deadline - now),
                },
            )
        });

        // 優先度の高い順、同順位は plugin の設定順と Card 識別子の順に並べる。
        let mut banners: Vec<&CardEntry> = self
            .cards
            .iter()
            .filter(|entry| entry.placement == Placement::Banner)
            .collect();
        banners.sort_by(|a, b| b.priority.cmp(&a.priority).then(a.key.cmp(&b.key)));

        let mut plan = Plan::default();
        let banner_notice = notice
            .as_ref()
            .filter(|(priority, _)| *priority != Priority::High);
        if let Some((_, shown)) = banner_notice {
            plan.bottom = Some(shown.clone());
            if !banners.is_empty() {
                plan.top = Some(Self::shown_card(banners[self.page % banners.len()], now));
            }
        } else {
            let pages = banners.len().div_ceil(2).max(1);
            let first = (self.page % pages) * 2;
            plan.top = banners.get(first).map(|entry| Self::shown_card(entry, now));
            plan.bottom = banners
                .get(first + 1)
                .map(|entry| Self::shown_card(entry, now));
        }

        // Overlay の候補は優先度、次に新しさで選ぶ。高優先度の通知は同順位の Card に勝つ。
        let overlay_card = self
            .cards
            .iter()
            .filter(|entry| entry.placement == Placement::Overlay)
            .max_by(|a, b| {
                a.priority
                    .cmp(&b.priority)
                    .then(a.revision.cmp(&b.revision))
            });
        plan.overlay = match (notice, overlay_card) {
            (Some((Priority::High, shown)), _) => Some(shown),
            (_, Some(entry)) => Some(Self::shown_card(entry, now)),
            _ => None,
        };
        plan
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(text: &str) -> Rows {
        let mut rows = Rows::new();
        let mut elements = heapless::Vec::new();
        elements
            .push(protocol::Element::Text {
                text: text.try_into().unwrap(),
            })
            .unwrap();
        rows.push(protocol::Row {
            elements,
            action: None,
        })
        .unwrap();
        rows
    }

    fn key(plugin: usize, card: u8) -> CardKey {
        CardKey { plugin, card }
    }

    fn card_key(shown: &Option<Shown>) -> Option<CardKey> {
        match shown.as_ref()?.source {
            Source::Card { key, .. } => Some(key),
            Source::Notice { .. } => None,
        }
    }

    fn put(scheduler: &mut Scheduler, key: CardKey, priority: Priority, now: Instant) {
        scheduler
            .put(key, Placement::Banner, priority, 0, rows("x"), 8, now)
            .unwrap();
    }

    #[test]
    fn banners_rotate_in_pages_of_two_by_priority() {
        let start = Instant::now();
        let mut scheduler = Scheduler::new(Duration::from_secs(10));
        put(&mut scheduler, key(0, 0), Priority::Low, start);
        put(&mut scheduler, key(1, 0), Priority::High, start);
        put(&mut scheduler, key(0, 1), Priority::Normal, start);

        let plan = scheduler.plan(start);
        assert_eq!(card_key(&plan.top), Some(key(1, 0)));
        assert_eq!(card_key(&plan.bottom), Some(key(0, 1)));
        assert!(plan.overlay.is_none());

        let plan = scheduler.plan(start + Duration::from_secs(9));
        assert_eq!(card_key(&plan.top), Some(key(1, 0)));

        let plan = scheduler.plan(start + Duration::from_secs(10));
        assert_eq!(card_key(&plan.top), Some(key(0, 0)));
        assert!(plan.bottom.is_none());

        let plan = scheduler.plan(start + Duration::from_secs(20));
        assert_eq!(card_key(&plan.top), Some(key(1, 0)));
    }

    #[test]
    fn replacing_a_card_changes_revision_but_not_position() {
        let now = Instant::now();
        let mut scheduler = Scheduler::new(Duration::from_secs(10));
        put(&mut scheduler, key(0, 0), Priority::Normal, now);
        let before = scheduler.plan(now).top.unwrap();
        put(&mut scheduler, key(0, 0), Priority::Normal, now);
        let after = scheduler.plan(now).top.unwrap();
        assert_ne!(before.source, after.source);
        assert_eq!(card_key(&Some(after)), Some(key(0, 0)));
    }

    #[test]
    fn card_limit_counts_other_cards_of_the_same_plugin() {
        let now = Instant::now();
        let mut scheduler = Scheduler::new(Duration::from_secs(10));
        let mut put_limited = |card: u8| {
            scheduler.put(
                key(0, card),
                Placement::Banner,
                Priority::Normal,
                0,
                rows("x"),
                2,
                now,
            )
        };
        assert!(put_limited(0).is_ok());
        assert!(put_limited(1).is_ok());
        assert!(put_limited(1).is_ok());
        assert!(put_limited(2).is_err());
        assert!(
            scheduler
                .put(
                    key(1, 0),
                    Placement::Banner,
                    Priority::Normal,
                    0,
                    rows("x"),
                    1,
                    now
                )
                .is_ok()
        );
    }

    #[test]
    fn expired_cards_disappear_and_overlay_requires_ttl() {
        let now = Instant::now();
        let mut scheduler = Scheduler::new(Duration::from_secs(10));
        assert!(
            scheduler
                .put(
                    key(0, 0),
                    Placement::Overlay,
                    Priority::Normal,
                    0,
                    rows("x"),
                    8,
                    now
                )
                .is_err()
        );
        scheduler
            .put(
                key(0, 0),
                Placement::Overlay,
                Priority::Normal,
                5,
                rows("x"),
                8,
                now,
            )
            .unwrap();
        let plan = scheduler.plan(now + Duration::from_secs(1));
        assert_eq!(card_key(&plan.overlay), Some(key(0, 0)));
        assert_eq!(
            plan.overlay.unwrap().remaining,
            Some(Duration::from_secs(4))
        );
        assert!(
            scheduler
                .plan(now + Duration::from_secs(5))
                .overlay
                .is_none()
        );
    }

    #[test]
    fn notices_interrupt_banners_or_overlay_by_priority_in_order() {
        let now = Instant::now();
        let mut scheduler = Scheduler::new(Duration::from_secs(10));
        put(&mut scheduler, key(0, 0), Priority::Normal, now);
        put(&mut scheduler, key(0, 1), Priority::Normal, now);
        scheduler.notify("first".into(), Priority::Normal, 3);
        scheduler.notify("urgent".into(), Priority::High, 0);

        let plan = scheduler.plan(now);
        assert_eq!(plan.bottom.unwrap().content, Content::Text("first".into()));
        assert_eq!(card_key(&plan.top), Some(key(0, 0)));
        assert!(plan.overlay.is_none());

        let plan = scheduler.plan(now + Duration::from_secs(3));
        assert_eq!(
            plan.overlay.unwrap().content,
            Content::Text("urgent".into())
        );
        assert_eq!(card_key(&plan.top), Some(key(0, 0)));
        assert_eq!(card_key(&plan.bottom), Some(key(0, 1)));

        let plan = scheduler.plan(now + Duration::from_secs(13));
        assert!(plan.overlay.is_none());
    }

    #[test]
    fn device_ids_roundtrip_and_reserve_zero() {
        assert_eq!(CardKey::from_device_id(0), None);
        for key in [key(0, 0), key(0, 7), key(15, 7), key(3, 2)] {
            assert_ne!(key.device_id(), 0);
            assert_eq!(CardKey::from_device_id(key.device_id()), Some(key));
        }
    }

    #[test]
    fn advance_moves_to_next_page_and_restarts_interval() {
        let now = Instant::now();
        let mut scheduler = Scheduler::new(Duration::from_secs(10));
        for card in 0..4 {
            put(&mut scheduler, key(0, card), Priority::Normal, now);
        }
        assert_eq!(card_key(&scheduler.plan(now).top), Some(key(0, 0)));
        scheduler.advance(now + Duration::from_secs(5));
        let plan = scheduler.plan(now + Duration::from_secs(5));
        assert_eq!(card_key(&plan.top), Some(key(0, 2)));
        // 送った時点から間隔を数え直す。
        let plan = scheduler.plan(now + Duration::from_secs(14));
        assert_eq!(card_key(&plan.top), Some(key(0, 2)));
    }

    #[test]
    fn removing_a_plugin_removes_its_cards() {
        let now = Instant::now();
        let mut scheduler = Scheduler::new(Duration::from_secs(10));
        put(&mut scheduler, key(0, 0), Priority::Normal, now);
        put(&mut scheduler, key(1, 0), Priority::Normal, now);
        scheduler.remove_plugin(0);
        let plan = scheduler.plan(now);
        assert_eq!(plan.visible_cards().collect::<Vec<_>>(), [key(1, 0)]);
        scheduler.remove(key(1, 0));
        assert_eq!(scheduler.plan(now), Plan::default());
    }
}
