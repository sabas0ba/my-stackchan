//! spool 接続の plugin (設定ディレクトリ内のファイル交換で daemon に接続する plugin)。
//!
//! Windows 側の PowerShell スクリプト等、daemon が起動できず socket も使えない plugin の
//! ための接続方式である。plugin は `<spool>/plugin/<id>/in/*.req` に要求を書き、daemon は
//! `out/*.evt` に通知を書く。書式は通知の受付 (spool.rs) と同じ行単位のテキストとする。
//!
//! daemon の内部では、ファイルから復元したメッセージを通常の plugin と同じ経路 (Hello、
//! 検証、流量制限、scheduler) に流す。そのために、ファイルの取込みと書出しを行う
//! スレッドを pipe で plugin の stdin / stdout に見せる。接続方式だけが異なり、plugin の
//! 扱いは分けない。設計は docs/plugin.md の「spool 接続の plugin」を参照する。

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use plugin_api::{
    API_VERSION, CardPut, Element, FrameReader, Hello, HostMessage, ImageFrame, LogLevel,
    Placement, PluginMessage, Priority, Row,
};

use super::plugin::{Process, Spawned};
use crate::config::{self, ConfigError, PluginConfig, Value};
use crate::log;

/// spool ディレクトリ内で、spool 接続の plugin のディレクトリを置く位置。
pub const DIR_NAME: &str = "plugin";
pub const IN_DIR: &str = "in";
pub const OUT_DIR: &str = "out";
/// `in/` を見る間隔。通知の受付と同じとする。
const POLL: Duration = Duration::from_millis(200);
/// 1 回の取込みで処理する件数。通知の受付と同じとする。
const MAX_PER_POLL: usize = crate::spool::MAX_PER_POLL;
const MAX_REQUEST_BYTES: u64 = 4096;
/// 画素ファイルの上限 (160×120 px の RGB565)。
const MAX_PIXELS_BYTES: u64 =
    plugin_api::MAX_IMAGE_WIDTH as u64 * plugin_api::MAX_IMAGE_HEIGHT as u64 * 2;
const MAX_DATA_NAME_BYTES: usize = 64;
/// `out/` に残す通知の上限。スクリプトが動いていない間に溜まり続けないようにする。
const EVENT_MAX_AGE: Duration = Duration::from_secs(60);
const EVENT_MAX_COUNT: usize = 64;
/// Card の行数の上限 (Overlay の行数)。
const MAX_ROWS: usize = 4;
const FILE: &str = "spool/plugin";
const REQUEST_EXTENSION: &str = "req";
const EVENT_EXTENSION: &str = "evt";

/// `in/` のファイル 1 件の内容。画像は画素ファイルを読んでからメッセージになる。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incoming {
    Message(PluginMessage),
    Image {
        width: u16,
        height: u16,
        ttl_s: u16,
        /// 同じディレクトリ内の画素ファイルの名前。
        data: String,
    },
}

type Entries = BTreeMap<String, (usize, Value)>;

const PLACEMENTS: [(&str, Placement); 2] = [
    ("banner", Placement::Banner),
    ("overlay", Placement::Overlay),
];
const PRIORITIES: [(&str, Priority); 3] = [
    ("low", Priority::Low),
    ("normal", Priority::Normal),
    ("high", Priority::High),
];

fn validate_header(header: &str) -> Result<(), &'static str> {
    match header {
        "card" | "card-remove" | "image" | "log" => Ok(()),
        _ => Err("要求は [card]、[card-remove]、[image]、[log] のいずれかです"),
    }
}

fn error(line: usize, reason: String) -> ConfigError {
    ConfigError {
        file: FILE.into(),
        line,
        reason,
    }
}

fn take_string(entries: &mut Entries, key: &str) -> Result<Option<String>, ConfigError> {
    match entries.remove(key) {
        None => Ok(None),
        Some((_, Value::String(value))) => Ok(Some(value)),
        Some((line, _)) => Err(error(line, format!("{key} は文字列です"))),
    }
}

/// 整数を `range` の範囲で読む。無ければ None。
fn take_integer(
    entries: &mut Entries,
    key: &str,
    range: std::ops::RangeInclusive<u32>,
) -> Result<Option<u32>, ConfigError> {
    match entries.remove(key) {
        None => Ok(None),
        Some((_, Value::Integer(value))) if range.contains(&value) => Ok(Some(value)),
        Some((line, _)) => Err(error(
            line,
            format!("{key} は {}..{} の整数です", range.start(), range.end()),
        )),
    }
}

fn require<T>(value: Option<T>, line: usize, key: &str) -> Result<T, ConfigError> {
    value.ok_or_else(|| error(line, format!("{key} が必要です")))
}

fn take_choice<T: Copy>(
    entries: &mut Entries,
    key: &str,
    choices: &[(&str, T)],
) -> Result<Option<T>, ConfigError> {
    let Some((line, value)) = entries.remove(key) else {
        return Ok(None);
    };
    let Value::String(value) = value else {
        return Err(error(line, format!("{key} は文字列です")));
    };
    choices
        .iter()
        .find(|(name, _)| *name == value)
        .map(|(_, choice)| Some(*choice))
        .ok_or_else(|| error(line, format!("未知の {key} です: {value}")))
}

/// `row<N>.*` の key から Card の行を組み立てる。行は 1 から連続していなければならない。
fn take_rows(entries: &mut Entries, line: usize) -> Result<Vec<Row>, ConfigError> {
    let mut rows = Vec::new();
    for number in 1..=MAX_ROWS {
        let key = |name: &str| format!("row{number}.{name}");
        let text = take_string(entries, &key("text"))?;
        let bar = take_integer(entries, &key("bar"), 0..=100)?;
        let label = take_string(entries, &key("label"))?;
        let spacer = take_integer(entries, &key("spacer"), 1..=32)?;
        let action = take_integer(entries, &key("action"), 0..=255)?.map(|value| value as u8);
        let element = match (text, bar, spacer) {
            (None, None, None) => {
                if label.is_some() || action.is_some() {
                    return Err(error(
                        line,
                        format!("row{number} には text、bar、spacer のいずれかが必要です"),
                    ));
                }
                break;
            }
            (Some(text), None, None) if label.is_none() => Element::Text { text },
            (None, Some(ratio), None) => Element::Bar {
                ratio: ratio as u8,
                label: label.unwrap_or_default(),
            },
            (None, None, Some(height)) if label.is_none() => Element::Spacer {
                height: height as u8,
            },
            _ => {
                return Err(error(
                    line,
                    format!(
                        "row{number} は text、bar (と label)、spacer のうち 1 つだけを持ちます"
                    ),
                ));
            }
        };
        rows.push(Row {
            elements: vec![element],
            action,
        });
    }
    if rows.is_empty() {
        return Err(error(line, "row1 が必要です".into()));
    }
    Ok(rows)
}

/// 画素ファイルの名前。同じディレクトリ内のファイルだけを指せるよう、path の区切りを許さない。
fn validate_data_name(name: &str) -> Result<(), &'static str> {
    let valid = !name.is_empty()
        && name.len() <= MAX_DATA_NAME_BYTES
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        && !name.starts_with('.');
    if valid {
        Ok(())
    } else {
        Err("data は英数字・._- からなる 64 byte 以内のファイル名です")
    }
}

/// 要求のファイルを解析する。section は 1 つだけとし、未知の key は誤りとする。
pub fn parse(text: &str) -> Result<Incoming, ConfigError> {
    let sections = config::parse_sections(FILE, text, validate_header)?;
    let [section] = <[_; 1]>::try_from(sections)
        .map_err(|_| error(0, "要求は 1 ファイルに 1 つだけ書きます".into()))?;
    let line = section.line;
    let mut entries = section.entries;
    let incoming = match section.header.as_str() {
        "card" => {
            let card = require(take_integer(&mut entries, "card", 0..=255)?, line, "card")?;
            let placement = require(
                take_choice(&mut entries, "placement", &PLACEMENTS)?,
                line,
                "placement",
            )?;
            let priority =
                take_choice(&mut entries, "priority", &PRIORITIES)?.unwrap_or(Priority::Normal);
            let ttl_s = take_integer(&mut entries, "ttl_s", 0..=u32::from(u16::MAX))?.unwrap_or(0);
            if placement == Placement::Overlay && ttl_s == 0 {
                return Err(error(line, "overlay の ttl_s は 1 以上です".into()));
            }
            let rows = take_rows(&mut entries, line)?;
            Incoming::Message(PluginMessage::CardPut(CardPut {
                card: card as u8,
                placement,
                priority,
                ttl_s: ttl_s as u16,
                rows,
            }))
        }
        "card-remove" => {
            let card = require(take_integer(&mut entries, "card", 0..=255)?, line, "card")?;
            Incoming::Message(PluginMessage::CardRemove { card: card as u8 })
        }
        "image" => {
            let width = require(
                take_integer(
                    &mut entries,
                    "width",
                    1..=u32::from(plugin_api::MAX_IMAGE_WIDTH),
                )?,
                line,
                "width",
            )?;
            let height = require(
                take_integer(
                    &mut entries,
                    "height",
                    1..=u32::from(plugin_api::MAX_IMAGE_HEIGHT),
                )?,
                line,
                "height",
            )?;
            let ttl_s = take_integer(&mut entries, "ttl_s", 0..=u32::from(u16::MAX))?.unwrap_or(0);
            let data = require(take_string(&mut entries, "data")?, line, "data")?;
            validate_data_name(&data).map_err(|reason| error(line, reason.into()))?;
            Incoming::Image {
                width: width as u16,
                height: height as u16,
                ttl_s: ttl_s as u16,
                data,
            }
        }
        _ => {
            let text = require(take_string(&mut entries, "text")?, line, "text")?;
            Incoming::Message(PluginMessage::Log {
                level: LogLevel::Info,
                text,
            })
        }
    };
    if let Some((key, (line, _))) = entries.into_iter().next() {
        return Err(error(line, format!("未知の key です: {key}")));
    }
    Ok(incoming)
}

/// 画素ファイルを読む。大きさは読む前に確かめ、不一致なら読まない。
fn read_pixels(path: &Path, width: u16, height: u16) -> Result<Vec<u8>, String> {
    let expected = u64::from(width) * u64::from(height) * 2;
    let size = fs::metadata(path)
        .map_err(|e| format!("画素ファイル {} を読めません: {e}", path.display()))?
        .len();
    if size > MAX_PIXELS_BYTES || size != expected {
        return Err(format!(
            "画素ファイルの大きさ ({size} byte) が幅 × 高さ × 2 ({expected} byte) と一致しません"
        ));
    }
    fs::read(path).map_err(|e| e.to_string())
}

fn read_request(path: &Path) -> Result<PluginMessage, String> {
    let size = fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > MAX_REQUEST_BYTES {
        return Err(format!("{MAX_REQUEST_BYTES} byte を超えています"));
    }
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    let text = String::from_utf8(bytes).map_err(|_| "UTF-8 ではありません".to_owned())?;
    match parse(&text).map_err(|e| e.to_string())? {
        Incoming::Message(message) => Ok(message),
        Incoming::Image {
            width,
            height,
            ttl_s,
            data,
        } => {
            // data は検証済みのファイル名であり、要求と同じディレクトリでだけ探す。
            let pixels_path = path.with_file_name(&data);
            let result = read_pixels(&pixels_path, width, height);
            // 拒否した場合も、同じ画素ファイルを残し続けないよう削除する。
            let _ = fs::remove_file(&pixels_path);
            Ok(PluginMessage::ImageFrame(ImageFrame {
                width,
                height,
                ttl_s,
                pixels: result?,
            }))
        }
    }
}

/// `in/` から名前の順に最大 `max` 件を取り込み、ファイルを削除する。不正なものも削除し、
/// 理由を返す。ディレクトリが無ければ何もしない。
pub fn drain(dir: &Path, max: usize) -> Vec<(String, Result<PluginMessage, String>)> {
    let mut paths = list(dir, REQUEST_EXTENSION);
    paths.sort();
    paths
        .into_iter()
        .take(max)
        .map(|path| {
            let name = file_name(&path);
            let request = read_request(&path);
            let request = match fs::remove_file(&path) {
                Ok(()) => request,
                Err(error) => Err(format!("処理後に削除できません: {error}")),
            };
            (name, request)
        })
        .collect()
}

fn list(dir: &Path, extension: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == extension) && path.is_file())
        .collect()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn quote(value: &str) -> String {
    // 書式のエスケープは \" と \\ だけであり、制御文字は表せないため空白に置き換える。
    let cleaned: String = value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    format!("\"{}\"", cleaned.replace('\\', "\\\\").replace('"', "\\\""))
}

/// daemon からの通知をファイルの内容にする。スクリプトに関係の無いものは None。
pub fn format_event(message: &HostMessage) -> Option<String> {
    Some(match message {
        HostMessage::Action { card, action } => {
            format!("[action]\ncard = {card}\naction = {action}\n")
        }
        HostMessage::Visibility { card, visible } => {
            format!("[visibility]\ncard = {card}\nvisible = {visible}\n")
        }
        HostMessage::Rejected { reason } => format!("[rejected]\nreason = {}\n", quote(reason)),
        HostMessage::Init(_) | HostMessage::Shutdown => return None,
    })
}

/// 通知を `out/` に書く。書きかけをスクリプトが読まないよう、一時ファイルに書いてから
/// rename する。名前は発生の順に並ぶようにする。
pub fn write_event(dir: &Path, message: &HostMessage) -> io::Result<Option<PathBuf>> {
    static SEQUENCE: AtomicU32 = AtomicU32::new(0);
    let Some(content) = format_event(message) else {
        return Ok(None);
    };
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let name = format!(
        "{millis:013}-{:010}",
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let temporary = dir.join(format!("{name}.tmp"));
    let path = dir.join(format!("{name}.{EVENT_EXTENSION}"));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    io::Write::write_all(&mut options.open(&temporary)?, content.as_bytes())?;
    fs::rename(&temporary, &path)?;
    Ok(Some(path))
}

/// 古い通知と、上限を超えた分の古い通知を `out/` から削除する。
pub fn prune_events(dir: &Path, now: SystemTime) {
    let mut paths = list(dir, EVENT_EXTENSION);
    paths.sort();
    let excess = paths.len().saturating_sub(EVENT_MAX_COUNT);
    for (index, path) in paths.iter().enumerate() {
        let expired = fs::metadata(path)
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > EVENT_MAX_AGE);
        if index < excess || expired {
            let _ = fs::remove_file(path);
        }
    }
}

/// 前回の daemon が残した通知を消す。起動時に呼ぶ。
fn clear_events(dir: &Path) {
    for path in list(dir, EVENT_EXTENSION)
        .into_iter()
        .chain(list(dir, "tmp"))
    {
        let _ = fs::remove_file(path);
    }
}

/// 取込みと書出しのスレッドを止める。plugin のプロセスは無いため、停止はこれだけである。
struct Stop(Arc<AtomicBool>);

impl Process for Stop {
    fn kill(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// spool 接続の plugin を「起動」する。
///
/// `in/` を読んでフレームにするスレッドと、daemon からのフレームを `out/` に書くスレッドを
/// 起こし、両者を pipe で結んだものを plugin の stdout / stdin として返す。最初のフレームは
/// 利用者設定の許可をそのまま要求する `Hello` とし、通常の plugin と同じ手順で動かす。
pub fn spawn(config: &PluginConfig, spool_dir: &Path) -> io::Result<Spawned> {
    let base = spool_dir.join(DIR_NAME).join(&config.id);
    let in_dir = base.join(IN_DIR);
    let out_dir = base.join(OUT_DIR);
    log::create_private_dir(&in_dir)?;
    log::create_private_dir(&out_dir)?;
    clear_events(&out_dir);

    let (daemon_reader, mut frames) = io::pipe()?;
    let (host_reader, daemon_writer) = io::pipe()?;
    let stop = Arc::new(AtomicBool::new(false));
    let hello = PluginMessage::Hello(Hello {
        api_version: API_VERSION,
        name: config.id.clone(),
        version: "spool".into(),
        capabilities: config.allowed,
    });

    let source = format!("plugin {} spool", config.id);
    {
        let stop = stop.clone();
        let in_dir = in_dir.clone();
        let out_dir = out_dir.clone();
        let source = source.clone();
        thread::spawn(move || {
            if plugin_api::write_frame(&mut frames, &hello).is_err() {
                return;
            }
            while !stop.load(Ordering::SeqCst) {
                for (name, request) in drain(&in_dir, MAX_PER_POLL) {
                    match request {
                        Ok(message) => {
                            if log::enabled(log::Level::Trace) {
                                log::trace!(source, "{name}: {message:?}");
                            }
                            if plugin_api::write_frame(&mut frames, &message).is_err() {
                                return;
                            }
                        }
                        Err(reason) => {
                            log::warning!(source, "{name} を捨てました: {reason}");
                            let rejected = HostMessage::Rejected {
                                reason: format!("{name}: {reason}"),
                            };
                            if let Err(error) = write_event(&out_dir, &rejected) {
                                log::warning!(source, "out に書けません: {error}");
                            }
                        }
                    }
                }
                prune_events(&out_dir, SystemTime::now());
                thread::sleep(POLL);
            }
        });
    }
    {
        let stop = stop.clone();
        thread::spawn(move || {
            let mut frames = FrameReader::new(host_reader);
            while let Ok(Some(message)) = frames.read_frame::<HostMessage>() {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                if let Err(error) = write_event(&out_dir, &message) {
                    log::warning!(source, "out に書けません: {error}");
                }
            }
        });
    }
    Ok(Spawned {
        stdin: Box::new(daemon_writer),
        stdout: Box::new(daemon_reader),
        stderr: None,
        process: Box::new(Stop(stop)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "stackchan-spool-plugin-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn card_request_becomes_card_put_with_single_element_rows() {
        let text = "\u{feff}[card]\r\ncard = 1\r\nplacement = \"overlay\"\r\npriority = \"high\"\r\nttl_s = 10\r\n\
                    row1.text = \"次の曲\"\r\nrow1.action = 3\r\n\
                    row2.bar = 40\r\nrow2.label = \"Aiobahn\"\r\nrow2.action = 2\r\n\
                    row3.spacer = 4\r\n";
        let Incoming::Message(PluginMessage::CardPut(card)) = parse(text).unwrap() else {
            panic!("CardPut になる");
        };
        assert_eq!(
            (card.card, card.placement, card.priority, card.ttl_s),
            (1, Placement::Overlay, Priority::High, 10)
        );
        assert_eq!(card.rows.len(), 3);
        assert_eq!(
            card.rows[0].elements,
            [Element::Text {
                text: "次の曲".into()
            }]
        );
        assert_eq!(card.rows[0].action, Some(3));
        assert_eq!(
            card.rows[1].elements,
            [Element::Bar {
                ratio: 40,
                label: "Aiobahn".into()
            }]
        );
        assert_eq!(card.rows[2].elements, [Element::Spacer { height: 4 }]);
        assert_eq!(card.rows[2].action, None);
        assert!(PluginMessage::CardPut(card).validate().is_ok());
    }

    #[test]
    fn malformed_card_requests_are_rejected() {
        for bad in [
            // 必須の key が無い
            "[card]\nplacement = \"banner\"\nrow1.text = \"a\"\n",
            "[card]\ncard = 0\nrow1.text = \"a\"\n",
            "[card]\ncard = 0\nplacement = \"banner\"\n",
            // overlay には ttl が要る
            "[card]\ncard = 0\nplacement = \"overlay\"\nrow1.text = \"a\"\n",
            // 行が連続していない
            "[card]\ncard = 0\nplacement = \"banner\"\nrow2.text = \"a\"\n",
            // 1 行に複数の要素
            "[card]\ncard = 0\nplacement = \"banner\"\nrow1.text = \"a\"\nrow1.bar = 1\n",
            // label は bar にだけ付く
            "[card]\ncard = 0\nplacement = \"banner\"\nrow1.text = \"a\"\nrow1.label = \"b\"\n",
            // 範囲外
            "[card]\ncard = 256\nplacement = \"banner\"\nrow1.text = \"a\"\n",
            "[card]\ncard = 0\nplacement = \"banner\"\nrow1.bar = 101\n",
            "[card]\ncard = 0\nplacement = \"banner\"\nrow1.spacer = 0\n",
            "[card]\ncard = 0\nplacement = \"banner\"\nrow1.text = \"a\"\nrow1.action = 256\n",
            // 未知の key と見出し
            "[card]\ncard = 0\nplacement = \"banner\"\nrow1.text = \"a\"\nrow5.text = \"b\"\n",
            "[card]\ncard = 0\nplacement = \"banner\"\nrow1.text = \"a\"\ncolor = \"red\"\n",
            "[notify]\ntext = \"a\"\n",
            // section が 2 つ
            "[card-remove]\ncard = 0\n[card-remove]\ncard = 1\n",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn remove_log_and_image_requests_are_parsed() {
        assert_eq!(
            parse("[card-remove]\ncard = 7\n").unwrap(),
            Incoming::Message(PluginMessage::CardRemove { card: 7 })
        );
        assert_eq!(
            parse("[log]\ntext = \"started\"\n").unwrap(),
            Incoming::Message(PluginMessage::Log {
                level: LogLevel::Info,
                text: "started".into()
            })
        );
        assert_eq!(
            parse("[image]\nwidth = 120\nheight = 120\nttl_s = 10\ndata = \"cover-1.rgb565\"\n")
                .unwrap(),
            Incoming::Image {
                width: 120,
                height: 120,
                ttl_s: 10,
                data: "cover-1.rgb565".into()
            }
        );
        for bad in [
            "[image]\nwidth = 161\nheight = 120\ndata = \"a\"\n",
            "[image]\nwidth = 0\nheight = 120\ndata = \"a\"\n",
            "[image]\nwidth = 120\nheight = 120\n",
            "[image]\nwidth = 120\nheight = 120\ndata = \"../a\"\n",
            "[image]\nwidth = 120\nheight = 120\ndata = \"sub/a\"\n",
            "[image]\nwidth = 120\nheight = 120\ndata = \"c:\\\\a\"\n",
            "[image]\nwidth = 120\nheight = 120\ndata = \".a\"\n",
            "[image]\nwidth = 120\nheight = 120\ndata = \"\"\n",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }

    fn write_request(dir: &Path, name: &str, content: &str) {
        let temporary = dir.join(format!("{name}.tmp"));
        fs::write(&temporary, content).unwrap();
        fs::rename(temporary, dir.join(format!("{name}.{REQUEST_EXTENSION}"))).unwrap();
    }

    #[test]
    fn drain_reads_pixels_next_to_the_request_and_removes_both() {
        let dir = temp_dir("drain");
        fs::write(dir.join("cover.rgb565"), [0u8; 2 * 3 * 2]).unwrap();
        write_request(
            &dir,
            "0001",
            "[image]\nwidth = 2\nheight = 3\ndata = \"cover.rgb565\"\n",
        );
        fs::write(dir.join("short.rgb565"), [0u8; 4]).unwrap();
        write_request(
            &dir,
            "0002",
            "[image]\nwidth = 2\nheight = 3\ndata = \"short.rgb565\"\n",
        );
        write_request(
            &dir,
            "0003",
            "[image]\nwidth = 2\nheight = 3\ndata = \"missing.rgb565\"\n",
        );
        // 書きかけ (.tmp) は読まない。
        fs::write(dir.join("0004.tmp"), "[card-remove]\ncard = 0\n").unwrap();
        let results = drain(&dir, 8);
        let names: Vec<&str> = results.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["0001.req", "0002.req", "0003.req"]);
        assert!(matches!(
            &results[0].1,
            Ok(PluginMessage::ImageFrame(frame)) if frame.width == 2 && frame.pixels.len() == 12
        ));
        assert!(
            results[1]
                .1
                .as_ref()
                .is_err_and(|e| e.contains("一致しません"))
        );
        assert!(
            results[2]
                .1
                .as_ref()
                .is_err_and(|e| e.contains("読めません"))
        );
        let remaining: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|entry| file_name(&entry.unwrap().path()))
            .collect();
        assert_eq!(
            remaining,
            ["0004.tmp"],
            "要求と画素ファイルは処理後に削除する"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn events_are_formatted_written_and_pruned() {
        assert_eq!(
            format_event(&HostMessage::Action { card: 1, action: 3 }).unwrap(),
            "[action]\ncard = 1\naction = 3\n"
        );
        assert_eq!(
            format_event(&HostMessage::Visibility {
                card: 0,
                visible: false
            })
            .unwrap(),
            "[visibility]\ncard = 0\nvisible = false\n"
        );
        assert_eq!(
            format_event(&HostMessage::Rejected {
                reason: "a \"b\"\n".into()
            })
            .unwrap(),
            "[rejected]\nreason = \"a \\\"b\\\" \"\n"
        );
        assert_eq!(format_event(&HostMessage::Shutdown), None);

        let dir = temp_dir("events");
        for action in 0..(EVENT_MAX_COUNT as u8 + 3) {
            write_event(&dir, &HostMessage::Action { card: 0, action }).unwrap();
        }
        assert!(write_event(&dir, &HostMessage::Shutdown).unwrap().is_none());
        prune_events(&dir, SystemTime::now());
        let mut remaining = list(&dir, EVENT_EXTENSION);
        remaining.sort();
        assert_eq!(
            remaining.len(),
            EVENT_MAX_COUNT,
            "上限を超えた古い分を削除する"
        );
        assert!(
            fs::read_to_string(&remaining[0])
                .unwrap()
                .contains("action = 3"),
            "残るのは新しい方"
        );
        prune_events(&dir, SystemTime::now() + EVENT_MAX_AGE * 2);
        assert!(list(&dir, EVENT_EXTENSION).is_empty(), "古い通知を削除する");
        fs::remove_dir_all(&dir).unwrap();
    }
}
