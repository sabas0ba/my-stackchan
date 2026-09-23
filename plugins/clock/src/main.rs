//! 時刻と日付を帯に表示する plugin。plugin API の参照実装を兼ねる。
//!
//! std はタイムゾーンを扱わないため、UTC からのずれ (分) を利用者設定の
//! `param.utc_offset_minutes` で受け取る。夏時間の切替えは扱わない。
//!
//! 時刻の行をタップすると (daemon の `input forward` 時)、現地時刻と UTC の表示を
//! 切り替える。行の action と `HostMessage::Action` の使い方の例を兼ねる。

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use plugin_api::client::{self, ClientError};
use plugin_api::{
    API_VERSION, Capabilities, CardPut, Element, Hello, HostMessage, LogLevel, Placement,
    PluginMessage, Priority, Row,
};

const CARD: u8 = 0;
/// 時刻の行のタップで返される値。
const TOGGLE_UTC: u8 = 0;
/// UTC からのずれの範囲。実在するタイムゾーンは -12:00..+14:00 に収まる。
const OFFSET_RANGE_MINUTES: std::ops::RangeInclusive<i64> = -12 * 60..=14 * 60;

/// 1970-01-01 からの日数を (年, 月, 日) に変換する。
///
/// 3 月始まりの暦で 400 年周期 (146097 日) に分解し、閏日を年の末尾に置いて計算する。
fn civil_date(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468; // 0000-03-01 からの日数
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153; // 3 月を 0 とする
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn weekday(days: i64) -> &'static str {
    // 1970-01-01 は木曜日。
    ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][days.rem_euclid(7) as usize]
}

/// UNIX 時刻 (秒) と UTC からのずれから、表示する時刻と日付の文字列を作る。
fn format_local(unix_seconds: i64, offset_minutes: i64) -> (String, String) {
    let local = unix_seconds + offset_minutes * 60;
    let days = local.div_euclid(86_400);
    let seconds_of_day = local.rem_euclid(86_400);
    let (year, month, day) = civil_date(days);
    (
        format!(
            "{:02}:{:02}",
            seconds_of_day / 3600,
            seconds_of_day / 60 % 60
        ),
        format!("{year:04}-{month:02}-{day:02} {}", weekday(days)),
    )
}

fn card(time: String, date: String) -> PluginMessage {
    // 帯の 1 行は 30 文字で、2 要素を等幅に置くため 1 要素 15 文字に収まる形式とする。
    PluginMessage::CardPut(CardPut {
        card: CARD,
        placement: Placement::Banner,
        priority: Priority::Low,
        ttl_s: 0,
        rows: vec![Row {
            elements: vec![Element::Text { text: time }, Element::Text { text: date }],
            action: Some(TOGGLE_UTC),
        }],
    })
}

fn parse_offset(value: Option<&str>) -> Result<i64, String> {
    let Some(value) = value else {
        return Ok(0);
    };
    let minutes: i64 = value
        .parse()
        .map_err(|_| format!("utc_offset_minutes が整数ではありません: {value}"))?;
    if !OFFSET_RANGE_MINUTES.contains(&minutes) {
        return Err(format!("utc_offset_minutes が範囲外です: {minutes}"));
    }
    Ok(minutes)
}

fn run() -> Result<(), ClientError> {
    let mut connection = client::connect_stdio(Hello {
        api_version: API_VERSION,
        name: "clock".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        capabilities: Capabilities {
            cards: 1,
            ..Capabilities::default()
        },
    })?;
    let offset = match parse_offset(connection.param("utc_offset_minutes")) {
        Ok(offset) => offset,
        Err(reason) => {
            connection.send(&PluginMessage::Log {
                level: LogLevel::Error,
                text: reason,
            })?;
            return Ok(());
        }
    };
    if connection.param("utc_offset_minutes").is_none() {
        connection.send(&PluginMessage::Log {
            level: LogLevel::Warn,
            text: "utc_offset_minutes が無いため UTC で表示します".into(),
        })?;
    }

    let mut show_utc = false;
    loop {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let (mut time, date) =
            format_local(now.as_secs() as i64, if show_utc { 0 } else { offset });
        if show_utc {
            time.push_str(" UTC");
        }
        connection.send(&card(time, date))?;
        // 次の分の境界まで待つ。待機中も Shutdown と接続の終了には応じる。
        let mut wait = Duration::from_secs(60 - now.as_secs() % 60)
            .saturating_sub(Duration::from_nanos(u64::from(now.subsec_nanos())));
        while !wait.is_zero() {
            let started = std::time::Instant::now();
            match connection.recv_timeout(wait)? {
                Some(HostMessage::Shutdown) => return Ok(()),
                Some(HostMessage::Rejected { reason }) => eprintln!("rejected: {reason}"),
                Some(HostMessage::Action {
                    card: CARD,
                    action: TOGGLE_UTC,
                }) => {
                    show_utc = !show_utc;
                    connection.send(&PluginMessage::Log {
                        level: LogLevel::Info,
                        text: format!("表示を切り替えました: utc={show_utc}"),
                    })?;
                    // 待ち時間を打ち切り、切り替えた表示をすぐに送る。
                    break;
                }
                _ if connection.is_closed() => return Ok(()),
                _ => {}
            }
            wait = wait.saturating_sub(started.elapsed());
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_date_matches_known_dates() {
        assert_eq!(civil_date(0), (1970, 1, 1));
        assert_eq!(civil_date(-1), (1969, 12, 31));
        // 閏年の 2 月 29 日と、100 年・400 年の規則。
        assert_eq!(civil_date(11_016), (2000, 2, 29));
        assert_eq!(civil_date(47_540), (2100, 2, 28));
        assert_eq!(civil_date(47_541), (2100, 3, 1));
        assert_eq!(civil_date(20_719), (2026, 9, 23));
    }

    #[test]
    fn local_time_applies_offset_across_day_boundary() {
        // 2026-09-23T15:30:00Z
        let unix = 1_790_177_400;
        assert_eq!(
            format_local(unix, 0),
            ("15:30".into(), "2026-09-23 Wed".into())
        );
        assert_eq!(
            format_local(unix, 9 * 60),
            ("00:30".into(), "2026-09-24 Thu".into())
        );
        assert_eq!(
            format_local(unix, -16 * 60),
            ("23:30".into(), "2026-09-22 Tue".into())
        );
    }

    #[test]
    fn offset_parameter_is_bounded() {
        assert_eq!(parse_offset(None), Ok(0));
        assert_eq!(parse_offset(Some("540")), Ok(540));
        assert_eq!(parse_offset(Some("-720")), Ok(-720));
        assert!(parse_offset(Some("841")).is_err());
        assert!(parse_offset(Some("9h")).is_err());
    }

    #[test]
    fn card_fits_the_banner_limits() {
        let (time, date) = format_local(0, 0);
        let PluginMessage::CardPut(card) = card(time, date) else {
            unreachable!()
        };
        assert_eq!(card.rows.len(), 1);
        for element in &card.rows[0].elements {
            let Element::Text { text } = element else {
                unreachable!()
            };
            // 帯の 1 行は 30 文字で、2 要素を等幅に置くため 1 要素 15 文字に収める。
            assert!(text.len() <= 15, "{text}");
        }
        assert_eq!(card.rows[0].action, Some(TOGGLE_UTC));
        let mut utc = format_local(0, 0).0;
        utc.push_str(" UTC");
        assert!(utc.len() <= 15, "{utc}");
    }
}
