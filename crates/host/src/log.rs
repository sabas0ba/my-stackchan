//! daemon のログ。
//!
//! 各行に UTC の時刻、重要度、発生元を付け、stderr と (設定した場合は) ファイルへ書く。
//! ログの crate を追加しないため自前で実装する。std はタイムゾーンを扱わないため、時刻は
//! UTC とする。ファイルは大きさの上限で世代を送り、容量が際限なく増えないようにする。
//!
//! 初期化しない場合 (CLI の単発の命令、試験) は stderr にだけ info 以上を書く。

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// 1 ファイルの上限。超えたら `.1` へ送る。
const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// 残す過去の世代の数 (`.1` .. `.N`)。
const KEEP_GENERATIONS: u32 = 2;
pub const FILE_NAME: &str = "daemon.log";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
    /// 送受信したメッセージ。`--trace` を指定した場合だけ出力する。
    Trace,
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Self::Error => "ERROR",
            Self::Warn => "WARN ",
            Self::Info => "INFO ",
            Self::Debug => "DEBUG",
            Self::Trace => "TRACE",
        }
    }
}

struct Sink {
    dir: PathBuf,
    file: File,
    written: u64,
}

struct Logger {
    max_level: Level,
    sink: Mutex<Option<Sink>>,
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

fn open(dir: &Path) -> std::io::Result<Sink> {
    fs::create_dir_all(dir)?;
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    // 他の利用者から読めないようにする。Windows では利用者のプロファイル配下の ACL に従う。
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let file = options.open(dir.join(FILE_NAME))?;
    let written = file.metadata()?.len();
    Ok(Sink {
        dir: dir.to_path_buf(),
        file,
        written,
    })
}

/// 世代を送る。`daemon.log.N` (最も古い) を消し、順に番号を 1 つずつ増やす。
fn rotate(sink: &mut Sink) -> std::io::Result<()> {
    let path = |generation: u32| match generation {
        0 => sink.dir.join(FILE_NAME),
        n => sink.dir.join(format!("{FILE_NAME}.{n}")),
    };
    // Windows の rename は移動先が存在すると失敗するため、先に消す。
    let _ = fs::remove_file(path(KEEP_GENERATIONS));
    for generation in (0..KEEP_GENERATIONS).rev() {
        let from = path(generation);
        if from.exists() {
            fs::rename(&from, path(generation + 1))?;
        }
    }
    *sink = open(&sink.dir)?;
    Ok(())
}

/// daemon の起動時に 1 回だけ呼ぶ。`dir` を与えた場合はファイルにも書く。
pub fn init(max_level: Level, dir: Option<&Path>) -> std::io::Result<()> {
    let sink = dir.map(open).transpose()?;
    LOGGER
        .set(Logger {
            max_level,
            sink: Mutex::new(sink),
        })
        .map_err(|_| std::io::Error::other("ログは初期化済みです"))
}

pub fn enabled(level: Level) -> bool {
    level <= LOGGER.get().map_or(Level::Info, |logger| logger.max_level)
}

/// 1 行を書く。`source` は `daemon`、`plugin <id>`、`device` 等の発生元。
pub fn write(level: Level, source: &str, message: fmt::Arguments<'_>) {
    if !enabled(level) {
        return;
    }
    let line = format!(
        "{} {} [{source}] {message}\n",
        utc_timestamp(SystemTime::now()),
        level.label()
    );
    // ログの出力の失敗で daemon を止めない。
    let _ = std::io::stderr().write_all(line.as_bytes());
    let Some(logger) = LOGGER.get() else {
        return;
    };
    let mut sink = logger
        .sink
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(current) = sink.as_mut() {
        if current.written + line.len() as u64 > MAX_FILE_BYTES
            && let Err(error) = rotate(current)
        {
            let _ = writeln!(std::io::stderr(), "ログの世代を送れません: {error}");
            *sink = None;
            return;
        }
        if current.file.write_all(line.as_bytes()).is_ok() {
            current.written += line.len() as u64;
        }
    }
}

macro_rules! error {
    ($source:expr, $($arg:tt)*) => { $crate::log::write($crate::log::Level::Error, &$source, format_args!($($arg)*)) };
}
macro_rules! warning {
    ($source:expr, $($arg:tt)*) => { $crate::log::write($crate::log::Level::Warn, &$source, format_args!($($arg)*)) };
}
macro_rules! info {
    ($source:expr, $($arg:tt)*) => { $crate::log::write($crate::log::Level::Info, &$source, format_args!($($arg)*)) };
}
macro_rules! debug {
    ($source:expr, $($arg:tt)*) => { $crate::log::write($crate::log::Level::Debug, &$source, format_args!($($arg)*)) };
}
macro_rules! trace {
    ($source:expr, $($arg:tt)*) => { $crate::log::write($crate::log::Level::Trace, &$source, format_args!($($arg)*)) };
}
pub(crate) use {debug, error, info, trace, warning};

/// 1970-01-01 からの日数を (年, 月, 日) に変換する。3 月始まりの暦で 400 年周期に分解する。
fn civil_date(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32;
    (year_of_era + era * 400 + i64::from(month <= 2), month, day)
}

/// RFC 3339 形式の UTC 時刻 (ミリ秒まで)。
pub fn utc_timestamp(time: SystemTime) -> String {
    let elapsed = time.duration_since(UNIX_EPOCH).unwrap_or_default();
    let seconds = elapsed.as_secs() as i64;
    let (year, month, day) = civil_date(seconds.div_euclid(86_400));
    let of_day = seconds.rem_euclid(86_400);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        of_day / 3600,
        of_day / 60 % 60,
        of_day % 60,
        elapsed.subsec_millis()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn timestamps_are_utc_rfc3339() {
        assert_eq!(utc_timestamp(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
        // 2026-09-23T15:30:00Z と、閏年の 2 月 29 日。Python の datetime で確認した値。
        assert_eq!(
            utc_timestamp(UNIX_EPOCH + Duration::from_millis(1_790_177_400_123)),
            "2026-09-23T15:30:00.123Z"
        );
        assert_eq!(
            utc_timestamp(UNIX_EPOCH + Duration::from_secs(11_016 * 86_400 + 86_399)),
            "2000-02-29T23:59:59.000Z"
        );
    }

    #[test]
    fn files_rotate_and_keep_limited_generations() {
        let dir = std::env::temp_dir().join(format!("stackchan-log-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let mut sink = open(&dir).unwrap();
        for generation in 0..4 {
            sink.file
                .write_all(format!("generation {generation}\n").as_bytes())
                .unwrap();
            rotate(&mut sink).unwrap();
        }
        let mut names: Vec<String> = fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["daemon.log", "daemon.log.1", "daemon.log.2"]);
        assert_eq!(
            fs::read_to_string(dir.join("daemon.log.1")).unwrap(),
            "generation 3\n"
        );
        assert_eq!(fs::read_to_string(dir.join("daemon.log")).unwrap(), "");
        fs::remove_dir_all(&dir).unwrap();
    }
}
