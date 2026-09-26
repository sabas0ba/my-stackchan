//! daemon が取り込む単発の要求 (spool)。
//!
//! daemon は port を占有するため、動作中の通知や活動状態の変更は、設定ディレクトリの
//! `spool/` に 1 件 1 ファイルで書いて渡す。Windows では利用者の build した exe を実行
//! できない場合があるため、形式は exe を使わずに書ける行単位のテキストとし、利用者設定と
//! 同じ書式 (section 見出しが要求の種類) とする。設計は docs/plugin.md の「通知と手動命令の
//! 受付 (spool)」を参照する。

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::{self, ConfigError, Value};

pub const DIR_NAME: &str = "spool";
/// daemon が 1 回の取込みで処理する件数。
pub const MAX_PER_POLL: usize = 8;
const MAX_REQUEST_BYTES: u64 = 4096;
const FILE: &str = "spool";
const REQUEST_EXTENSION: &str = "req";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Notify {
        text: String,
        priority: plugin_api::Priority,
        ttl_s: u16,
    },
    Status {
        activity: protocol::Activity,
        detail: String,
        ttl_s: u16,
    },
    Input(protocol::InputMode),
}

impl Request {
    /// ログ用の要約。本文と詳細は含めず、長さだけを出す。
    pub fn summary(&self) -> String {
        match self {
            Self::Notify {
                text,
                priority,
                ttl_s,
            } => format!(
                "notify (priority={}, ttl_s={ttl_s}, text={} byte)",
                name_of(&PRIORITIES, priority),
                text.len()
            ),
            Self::Status {
                activity,
                detail,
                ttl_s,
            } => format!(
                "status (activity={}, ttl_s={ttl_s}, detail={} byte)",
                name_of(&ACTIVITIES, activity),
                detail.len()
            ),
            Self::Input(mode) => format!("input (mode={})", name_of(&MODES, mode)),
        }
    }
}

fn validate_header(header: &str) -> Result<(), &'static str> {
    match header {
        "notify" | "status" | "input" => Ok(()),
        _ => Err("要求は [notify]、[status]、[input] のいずれかです"),
    }
}

const PRIORITIES: [(&str, plugin_api::Priority); 3] = [
    ("low", plugin_api::Priority::Low),
    ("normal", plugin_api::Priority::Normal),
    ("high", plugin_api::Priority::High),
];
const ACTIVITIES: [(&str, protocol::Activity); 5] = [
    ("idle", protocol::Activity::Idle),
    ("working", protocol::Activity::Working),
    ("waiting", protocol::Activity::Waiting),
    ("done", protocol::Activity::Done),
    ("error", protocol::Activity::Error),
];
const MODES: [(&str, protocol::InputMode); 2] = [
    ("demo", protocol::InputMode::Demo),
    ("forward", protocol::InputMode::Forward),
];

fn name_of<T: PartialEq>(choices: &[(&'static str, T)], value: &T) -> &'static str {
    choices
        .iter()
        .find(|(_, choice)| choice == value)
        .map_or("", |(name, _)| name)
}

type Entries = BTreeMap<String, (usize, Value)>;

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

fn take_ttl(entries: &mut Entries, default: u16) -> Result<u16, ConfigError> {
    match entries.remove("ttl_s") {
        None => Ok(default),
        Some((line, Value::Integer(value))) => {
            u16::try_from(value).map_err(|_| error(line, "ttl_s は 0..65535 です".into()))
        }
        Some((line, _)) => Err(error(line, "ttl_s は整数です".into())),
    }
}

/// 必須の文字列を、許す値の表から選ぶ。
fn take_choice<T: Copy>(
    entries: &mut Entries,
    key: &str,
    line: usize,
    choices: &[(&str, T)],
    default: Option<T>,
) -> Result<T, ConfigError> {
    match take_string(entries, key)? {
        None => default.ok_or_else(|| error(line, format!("{key} が必要です"))),
        Some(value) => choices
            .iter()
            .find(|(name, _)| *name == value)
            .map(|(_, choice)| *choice)
            .ok_or_else(|| error(line, format!("未知の {key} です: {value}"))),
    }
}

/// 要求のファイルを解析する。section は 1 つだけとし、未知の key は誤りとする。
pub fn parse(text: &str) -> Result<Request, ConfigError> {
    let sections = config::parse_sections(FILE, text, validate_header)?;
    let [section] = <[_; 1]>::try_from(sections)
        .map_err(|_| error(0, "要求は 1 ファイルに 1 つだけ書きます".into()))?;
    let line = section.line;
    let mut entries = section.entries;
    let request = match section.header.as_str() {
        "notify" => Request::Notify {
            text: take_string(&mut entries, "text")?
                .filter(|text| !text.is_empty())
                .ok_or_else(|| error(line, "text が必要です".into()))?,
            priority: take_choice(
                &mut entries,
                "priority",
                line,
                &PRIORITIES,
                Some(plugin_api::Priority::Normal),
            )?,
            ttl_s: take_ttl(&mut entries, 0)?,
        },
        "status" => Request::Status {
            activity: take_choice(&mut entries, "activity", line, &ACTIVITIES, None)?,
            detail: take_string(&mut entries, "detail")?.unwrap_or_default(),
            ttl_s: take_ttl(&mut entries, 30)?,
        },
        _ => Request::Input(take_choice(&mut entries, "mode", line, &MODES, None)?),
    };
    if let Some((key, (line, _))) = entries.into_iter().next() {
        return Err(error(line, format!("未知の key です: {key}")));
    }
    Ok(request)
}

/// 文字列の値を書式に合わせて引用する。制御文字は書式が許さないため拒否する。
fn quote(value: &str) -> Result<String, &'static str> {
    if value.chars().any(char::is_control) {
        return Err("制御文字 (改行等) は含められません");
    }
    Ok(format!(
        "\"{}\"",
        value.replace('\\', "\\\\").replace('"', "\\\"")
    ))
}

/// 要求をファイルの内容にする。`parse` の逆変換である。
pub fn format(request: &Request) -> Result<String, &'static str> {
    Ok(match request {
        Request::Notify {
            text,
            priority,
            ttl_s,
        } => format!(
            "[notify]\ntext = {}\npriority = \"{}\"\nttl_s = {ttl_s}\n",
            quote(text)?,
            name_of(&PRIORITIES, priority)
        ),
        Request::Status {
            activity,
            detail,
            ttl_s,
        } => format!(
            "[status]\nactivity = \"{}\"\ndetail = {}\nttl_s = {ttl_s}\n",
            name_of(&ACTIVITIES, activity),
            quote(detail)?
        ),
        Request::Input(mode) => format!("[input]\nmode = \"{}\"\n", name_of(&MODES, mode)),
    })
}

/// 要求を spool に書く。書きかけのファイルを daemon が読まないよう、一時ファイルに
/// 書いてから rename する。名前は時刻の順に並ぶようにする。
pub fn write(dir: &Path, request: &Request) -> io::Result<PathBuf> {
    static SEQUENCE: AtomicU32 = AtomicU32::new(0);
    let content = format(request).map_err(io::Error::other)?;
    crate::log::create_private_dir(dir)?;
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let name = format!(
        "{millis:013}-{:010}-{:010}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let temporary = dir.join(format!("{name}.tmp"));
    let path = dir.join(format!("{name}.{REQUEST_EXTENSION}"));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    io::Write::write_all(&mut options.open(&temporary)?, content.as_bytes())?;
    fs::rename(&temporary, &path)?;
    Ok(path)
}

fn read(path: &Path) -> Result<Request, String> {
    let size = fs::metadata(path).map_err(|e| e.to_string())?.len();
    if size > MAX_REQUEST_BYTES {
        return Err(format!("{MAX_REQUEST_BYTES} byte を超えています"));
    }
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    let text = String::from_utf8(bytes).map_err(|_| "UTF-8 ではありません".to_owned())?;
    parse(&text).map_err(|e| e.to_string())
}

/// spool から名前の順に最大 `max` 件を取り込み、ファイルを削除する。不正なものも削除し、
/// 理由を返す (呼出し側がログに残す)。ディレクトリが無ければ何もしない。
pub fn drain(dir: &Path, max: usize) -> Vec<(String, Result<Request, String>)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == REQUEST_EXTENSION) && path.is_file()
        })
        .collect();
    paths.sort();
    paths
        .into_iter()
        .take(max)
        .map(|path| {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let request = read(&path);
            // 同じ要求を繰り返し処理しないよう、削除できない場合も結果に含める。
            let request = match fs::remove_file(&path) {
                Ok(()) => request,
                Err(error) => Err(format!("処理後に削除できません: {error}")),
            };
            (name, request)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("stackchan-spool-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn requests_roundtrip_through_the_file_format() {
        for request in [
            Request::Notify {
                text: "Claude Code: \"入力\" を待っています \\".into(),
                priority: plugin_api::Priority::High,
                ttl_s: 10,
            },
            Request::Status {
                activity: protocol::Activity::Working,
                detail: "BUILD".into(),
                ttl_s: 0,
            },
            Request::Input(protocol::InputMode::Forward),
        ] {
            let text = format(&request).unwrap();
            assert_eq!(parse(&text).unwrap(), request, "{text}");
        }
    }

    #[test]
    fn summary_does_not_include_message_contents() {
        let notify = Request::Notify {
            text: "project-x build failed".into(),
            priority: plugin_api::Priority::High,
            ttl_s: 5,
        };
        let status = Request::Status {
            activity: protocol::Activity::Waiting,
            detail: "PRIVATE".into(),
            ttl_s: 30,
        };
        assert_eq!(
            notify.summary(),
            "notify (priority=high, ttl_s=5, text=22 byte)"
        );
        assert!(!status.summary().contains("PRIVATE"));
    }

    #[test]
    fn powershell_output_with_bom_and_crlf_is_accepted() {
        let text = "\u{feff}[notify]\r\ntext = \"hook\"\r\n";
        assert_eq!(
            parse(text).unwrap(),
            Request::Notify {
                text: "hook".into(),
                priority: plugin_api::Priority::Normal,
                ttl_s: 0
            }
        );
    }

    #[test]
    fn invalid_requests_are_rejected() {
        for text in [
            "",
            "[notify]\n",
            "[notify]\ntext = \"\"\n",
            "[notify]\ntext = \"a\"\nunknown = 1\n",
            "[notify]\ntext = \"a\"\npriority = \"urgent\"\n",
            "[notify]\ntext = \"a\"\nttl_s = 70000\n",
            "[notify]\ntext = \"a\"\n[input]\nmode = \"demo\"\n",
            "[status]\ndetail = \"x\"\n",
            "[input]\nmode = \"auto\"\n",
            "[daemon]\n",
        ] {
            assert!(parse(text).is_err(), "{text:?}");
        }
        assert!(
            format(&Request::Notify {
                text: "two\nlines".into(),
                priority: plugin_api::Priority::Normal,
                ttl_s: 0
            })
            .is_err()
        );
    }

    #[test]
    fn written_requests_are_drained_in_order_and_removed() {
        let dir = temp_dir("drain");
        let requests: Vec<Request> = (0..10)
            .map(|index| Request::Notify {
                text: format!("n{index}"),
                priority: plugin_api::Priority::Normal,
                ttl_s: 0,
            })
            .collect();
        for request in &requests {
            write(&dir, request).unwrap();
        }
        // 書きかけのファイルと不正なファイルは、読まない / 理由付きで削除する。
        fs::write(dir.join("9999999999999-0-0.tmp"), "[notify]\n").unwrap();
        fs::write(dir.join("9999999999999-0-1.req"), "[unknown]\n").unwrap();

        let first = drain(&dir, MAX_PER_POLL);
        assert_eq!(first.len(), MAX_PER_POLL);
        for (index, (_, request)) in first.iter().enumerate() {
            assert_eq!(request.as_ref().unwrap(), &requests[index]);
        }
        let second = drain(&dir, MAX_PER_POLL);
        assert_eq!(second.len(), 3);
        assert!(second[2].1.is_err(), "不正な要求は理由付きで返る");
        assert!(drain(&dir, MAX_PER_POLL).is_empty());
        assert!(dir.join("9999999999999-0-0.tmp").exists(), "書きかけは残す");
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn oversized_and_non_utf8_files_are_rejected() {
        let dir = temp_dir("limits");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("a.req"),
            vec![b'#'; MAX_REQUEST_BYTES as usize + 1],
        )
        .unwrap();
        fs::write(dir.join("b.req"), [0xFF, 0xFE]).unwrap();
        let drained = drain(&dir, MAX_PER_POLL);
        assert_eq!(drained.len(), 2);
        assert!(drained.iter().all(|(_, request)| request.is_err()));
        fs::remove_dir_all(&dir).unwrap();
    }
}
