//! 利用者設定の読込み。
//!
//! TOML 等の crate を追加しないため、行単位の簡易な形式を自前で解析する。受け付ける
//! 構文は section 見出し、`key = 値` の行、`#` で始まるコメント行に限る。値は文字列、
//! 整数、真偽値、文字列の配列のみとし、未知の key と重複は誤りとする。
//!
//! secret は `stackchan.conf` とは別の `secrets.conf` に置く。前者を共有・版管理しても
//! secret が混入しないようにするためである。両者は同じ構文で、`secrets.conf` には
//! `[plugin <id>]` の `param.*` だけを書ける。

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub const CONFIG_FILE: &str = "stackchan.conf";
pub const SECRETS_FILE: &str = "secrets.conf";

const MAX_FILE_BYTES: usize = 64 * 1024;
const MAX_LINE_BYTES: usize = 4096;
const MAX_PLUGINS: usize = 16;
const MAX_ARGV: usize = 64;
const MAX_ID_BYTES: usize = 32;
const DEFAULT_ROTATE_S: u32 = 10;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub daemon: DaemonConfig,
    pub plugins: Vec<PluginConfig>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DaemonConfig {
    /// 省略時は VID:PID から自動検出する。
    pub port: Option<String>,
    /// 帯の Card を切り替える間隔。
    pub rotate_s: u32,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            port: None,
            rotate_s: DEFAULT_ROTATE_S,
        }
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct PluginConfig {
    pub id: String,
    /// 起動する argv。先頭が実行ファイル。隔離コマンドを前置きしてもよい。
    pub command: Vec<String>,
    /// 取得・確認した commit SHA。ログへの記録に用いる。
    pub rev: Option<String>,
    pub allowed: plugin_api::Capabilities,
    /// `param.*` と `secrets.conf` の値。plugin へ `Init` で渡す。
    pub params: Vec<(String, String)>,
    /// `env.*`。plugin に追加で渡す環境変数。daemon の環境変数は許可したものしか渡さない
    /// (daemon/plugin.rs)。環境変数はプロセスの情報から見え得るため、secret は `param` で渡す。
    pub env: Vec<(String, String)>,
}

/// Debug は params と env の値を出さない。secret を含み得るため。
impl std::fmt::Debug for PluginConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names = |pairs: &[(String, String)]| -> Vec<String> {
            pairs.iter().map(|(name, _)| name.clone()).collect()
        };
        f.debug_struct("PluginConfig")
            .field("id", &self.id)
            .field("command", &self.command)
            .field("rev", &self.rev)
            .field("allowed", &self.allowed)
            .field("params", &names(&self.params))
            .field("env", &names(&self.env))
            .finish()
    }
}

/// `stackchan config` の要約。param と env は名前だけを出す。
pub fn summary(dir: &Path, config: &Config) -> String {
    let mut lines = vec![
        format!("config dir: {}", dir.display()),
        format!(
            "daemon: port={}, rotate_s={}",
            config.daemon.port.as_deref().unwrap_or("(auto)"),
            config.daemon.rotate_s
        ),
    ];
    for plugin in &config.plugins {
        let allowed = plugin.allowed;
        lines.push(format!(
            "plugin {}: command={:?}, rev={}, cards={}, notify={}, presence={}, motion={}",
            plugin.id,
            plugin.command,
            plugin.rev.as_deref().unwrap_or("-"),
            allowed.cards,
            allowed.notify,
            allowed.presence,
            allowed.motion
        ));
        let names = |pairs: &[(String, String)]| -> Vec<String> {
            pairs.iter().map(|(name, _)| name.clone()).collect()
        };
        lines.push(format!("  params: {:?}", names(&plugin.params)));
        lines.push(format!("  env: {:?}", names(&plugin.env)));
    }
    lines.join("\n")
}

const MAX_ENV: usize = 16;

fn push_env(plugin: &mut PluginConfig, key: &str, value: String) -> Result<(), &'static str> {
    let name = &key["env.".len()..];
    let valid = !name.is_empty()
        && name.len() <= 64
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
        && !name.as_bytes()[0].is_ascii_digit();
    if !valid {
        return Err("env の名前は英字・数字・_ で、数字で始めません");
    }
    if value.len() > plugin_api::MAX_PARAM_VALUE_BYTES {
        return Err("env の値が長すぎます");
    }
    if plugin.env.len() == MAX_ENV {
        return Err("env の数が上限を超えます");
    }
    plugin.env.push((name.into(), value));
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigError {
    pub file: String,
    pub line: usize,
    pub reason: String,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.line == 0 {
            write!(f, "{}: {}", self.file, self.reason)
        } else {
            write!(f, "{}:{}: {}", self.file, self.line, self.reason)
        }
    }
}

impl std::error::Error for ConfigError {}

/// `--config-dir` が無い場合の設定ディレクトリ。`env` は環境変数の参照 (試験で差し替える)。
pub fn default_dir(
    windows: bool,
    env: impl Fn(&str) -> Option<OsString>,
) -> Result<PathBuf, String> {
    let absolute = |value: OsString| {
        let path = PathBuf::from(value);
        path.is_absolute().then_some(path)
    };
    let base = if windows {
        env("APPDATA").and_then(absolute)
    } else {
        env("XDG_CONFIG_HOME").and_then(absolute).or_else(|| {
            env("HOME")
                .and_then(absolute)
                .map(|home| home.join(".config"))
        })
    };
    base.map(|base| base.join("stackchan")).ok_or_else(|| {
        let names = if windows {
            "APPDATA"
        } else {
            "XDG_CONFIG_HOME / HOME"
        };
        format!("設定ディレクトリを決められません ({names} が絶対 path ではありません)。--config-dir を指定してください")
    })
}

pub fn resolve_dir(explicit: Option<PathBuf>) -> Result<PathBuf, String> {
    match explicit {
        Some(dir) => Ok(dir),
        None => default_dir(cfg!(windows), |name| std::env::var_os(name)),
    }
}

/// 設定ディレクトリから `stackchan.conf` と、存在すれば `secrets.conf` を読む。
pub fn load(dir: &Path) -> Result<Config, Box<dyn std::error::Error>> {
    let read = |name: &str, required: bool| -> Result<Option<String>, Box<dyn std::error::Error>> {
        let path = dir.join(name);
        match std::fs::metadata(&path) {
            Ok(meta) if meta.len() > MAX_FILE_BYTES as u64 => {
                Err(format!("{}: {MAX_FILE_BYTES} byte を超えています", path.display()).into())
            }
            Ok(_) => Ok(Some(std::fs::read_to_string(&path)?)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !required => Ok(None),
            Err(error) => Err(format!("{}: {error}", path.display()).into()),
        }
    };
    let config = read(CONFIG_FILE, true)?.unwrap_or_default();
    let secrets = read(SECRETS_FILE, false)?;
    Ok(parse(&config, secrets.as_deref())?)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Value {
    String(String),
    Integer(u32),
    Bool(bool),
    Strings(Vec<String>),
}

#[derive(Debug)]
pub(crate) struct Section {
    pub(crate) header: String,
    pub(crate) line: usize,
    pub(crate) entries: BTreeMap<String, (usize, Value)>,
}

pub fn parse(config: &str, secrets: Option<&str>) -> Result<Config, ConfigError> {
    let sections = parse_sections(CONFIG_FILE, config, validate_header)?;
    let mut result = Config {
        daemon: DaemonConfig::default(),
        plugins: Vec::new(),
    };
    let mut seen_daemon = false;
    for section in sections {
        let error = |line: usize, reason: String| ConfigError {
            file: CONFIG_FILE.into(),
            line,
            reason,
        };
        if section.header == "daemon" {
            if seen_daemon {
                return Err(error(section.line, "[daemon] が重複しています".into()));
            }
            seen_daemon = true;
            for (key, (line, value)) in section.entries {
                match (key.as_str(), value) {
                    ("port", Value::String(port)) => result.daemon.port = Some(port),
                    ("rotate_s", Value::Integer(seconds)) if (1..=3600).contains(&seconds) => {
                        result.daemon.rotate_s = seconds;
                    }
                    ("port" | "rotate_s", _) => {
                        return Err(error(line, format!("{key} の値が不正です")));
                    }
                    _ => return Err(error(line, format!("未知の key です: {key}"))),
                }
            }
        } else if let Some(id) = section.header.strip_prefix("plugin ").map(str::to_owned) {
            if result.plugins.iter().any(|plugin| plugin.id == id) {
                return Err(error(section.line, format!("plugin {id} が重複しています")));
            }
            if result.plugins.len() == MAX_PLUGINS {
                return Err(error(
                    section.line,
                    format!("plugin は {MAX_PLUGINS} 個までです"),
                ));
            }
            result.plugins.push(plugin_section(&id, section)?);
        } else {
            return Err(error(
                section.line,
                format!("未知の section です: [{}]", section.header),
            ));
        }
    }
    if let Some(secrets) = secrets {
        merge_secrets(&mut result, secrets)?;
    }
    Ok(result)
}

fn plugin_section(id: &str, section: Section) -> Result<PluginConfig, ConfigError> {
    let error = |line: usize, reason: String| ConfigError {
        file: CONFIG_FILE.into(),
        line,
        reason,
    };
    let mut plugin = PluginConfig {
        id: id.into(),
        command: Vec::new(),
        rev: None,
        allowed: plugin_api::Capabilities::default(),
        params: Vec::new(),
        env: Vec::new(),
    };
    for (key, (line, value)) in section.entries {
        match (key.as_str(), value) {
            ("command", Value::Strings(argv))
                if !argv.is_empty() && argv.len() <= MAX_ARGV && !argv[0].is_empty() =>
            {
                plugin.command = argv;
            }
            ("rev", Value::String(rev))
                if rev.len() == 40 && rev.bytes().all(|b| b.is_ascii_hexdigit()) =>
            {
                plugin.rev = Some(rev.to_ascii_lowercase());
            }
            ("cards", Value::Integer(cards)) if cards <= u32::from(plugin_api::MAX_CARDS) => {
                plugin.allowed.cards = cards as u8;
            }
            ("notify", Value::Bool(value)) => plugin.allowed.notify = value,
            ("presence", Value::Bool(value)) => plugin.allowed.presence = value,
            ("motion", Value::Bool(value)) => plugin.allowed.motion = value,
            (key, Value::String(value)) if key.starts_with("env.") => {
                push_env(&mut plugin, key, value).map_err(|reason| error(line, reason.into()))?;
            }
            (key, Value::String(value)) if key.starts_with("param.") => {
                push_param(&mut plugin, key, value).map_err(|reason| error(line, reason.into()))?;
            }
            ("command" | "rev" | "cards" | "notify" | "presence" | "motion", _) => {
                return Err(error(line, format!("{key} の値が不正です")));
            }
            _ => return Err(error(line, format!("未知の key です: {key}"))),
        }
    }
    if plugin.command.is_empty() {
        return Err(error(
            section.line,
            format!("plugin {id} に command がありません"),
        ));
    }
    Ok(plugin)
}

fn push_param(plugin: &mut PluginConfig, key: &str, value: String) -> Result<(), &'static str> {
    let name = &key["param.".len()..];
    if name.is_empty() || name.len() > plugin_api::MAX_PARAM_KEY_BYTES {
        return Err("param の名前が不正です");
    }
    if value.len() > plugin_api::MAX_PARAM_VALUE_BYTES {
        return Err("param の値が長すぎます");
    }
    if plugin.params.iter().any(|(existing, _)| existing == name) {
        return Err("param が stackchan.conf と secrets.conf で重複しています");
    }
    if plugin.params.len() == plugin_api::MAX_PARAMS {
        return Err("param の数が上限を超えます");
    }
    plugin.params.push((name.into(), value));
    Ok(())
}

fn merge_secrets(config: &mut Config, secrets: &str) -> Result<(), ConfigError> {
    for section in parse_sections(SECRETS_FILE, secrets, validate_header)? {
        let error = |line: usize, reason: String| ConfigError {
            file: SECRETS_FILE.into(),
            line,
            reason,
        };
        let Some(plugin) = section
            .header
            .strip_prefix("plugin ")
            .and_then(|id| config.plugins.iter_mut().find(|plugin| plugin.id == id))
        else {
            return Err(error(
                section.line,
                format!("stackchan.conf に無い section です: [{}]", section.header),
            ));
        };
        for (key, (line, value)) in section.entries {
            match value {
                Value::String(value) if key.starts_with("param.") => {
                    push_param(plugin, &key, value).map_err(|reason| error(line, reason.into()))?
                }
                _ => {
                    return Err(error(
                        line,
                        format!("secrets.conf に書けない key です: {key}"),
                    ));
                }
            }
        }
    }
    Ok(())
}

/// 行単位の書式を section ごとに解析する。見出しの検査は呼出し側が与える
/// (利用者設定と spool の要求で、許す見出しが異なるため)。
pub(crate) fn parse_sections(
    file: &str,
    text: &str,
    validate_header: fn(&str) -> Result<(), &'static str>,
) -> Result<Vec<Section>, ConfigError> {
    let error = |line: usize, reason: &str| ConfigError {
        file: file.into(),
        line,
        reason: reason.into(),
    };
    if text.len() > MAX_FILE_BYTES {
        return Err(error(0, "ファイルが大きすぎます"));
    }
    // Windows の PowerShell 5.1 は UTF-8 の先頭に BOM を付けるため、読み飛ばす。
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut sections: Vec<Section> = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let number = index + 1;
        if raw.len() > MAX_LINE_BYTES {
            return Err(error(number, "行が長すぎます"));
        }
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(header) = line.strip_prefix('[') {
            let header = header
                .strip_suffix(']')
                .ok_or_else(|| error(number, "section 見出しが ] で終わっていません"))?;
            validate_header(header).map_err(|reason| error(number, reason))?;
            sections.push(Section {
                header: header.into(),
                line: number,
                entries: BTreeMap::new(),
            });
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| error(number, "key = 値 の形式ではありません"))?;
        let key = key.trim();
        // 環境変数の名前は大文字を含むため、env. の後に限り英大文字も許す。
        let (prefix, rest) = key.split_at(if key.starts_with("env.") { 4 } else { 0 });
        let allowed = |b: u8, upper: bool| {
            b.is_ascii_lowercase()
                || b.is_ascii_digit()
                || b"_.".contains(&b)
                || (upper && b.is_ascii_uppercase())
        };
        if key.is_empty()
            || !prefix.bytes().all(|b| allowed(b, false))
            || !rest.bytes().all(|b| allowed(b, !prefix.is_empty()))
        {
            return Err(error(number, "key に使えない文字があります"));
        }
        let value = parse_value(value.trim()).map_err(|reason| error(number, reason))?;
        let section = sections
            .last_mut()
            .ok_or_else(|| error(number, "section の前に key があります"))?;
        if section
            .entries
            .insert(key.into(), (number, value))
            .is_some()
        {
            return Err(error(number, "key が重複しています"));
        }
    }
    Ok(sections)
}

fn validate_header(header: &str) -> Result<(), &'static str> {
    if header == "daemon" {
        return Ok(());
    }
    let id = header
        .strip_prefix("plugin ")
        .ok_or("section は [daemon] か [plugin <id>] です")?;
    if id.is_empty()
        || id.len() > MAX_ID_BYTES
        || !id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Err("plugin の id は英小文字・数字・- の 1..32 文字です");
    }
    Ok(())
}

fn parse_value(text: &str) -> Result<Value, &'static str> {
    match text {
        "true" => return Ok(Value::Bool(true)),
        "false" => return Ok(Value::Bool(false)),
        _ => {}
    }
    if text.starts_with('"') {
        let (value, rest) = parse_string(text)?;
        return if rest.trim().is_empty() {
            Ok(Value::String(value))
        } else {
            Err("文字列の後に余分な文字があります")
        };
    }
    if let Some(mut rest) = text.strip_prefix('[') {
        let mut items = Vec::new();
        loop {
            rest = rest.trim_start();
            if let Some(after) = rest.strip_prefix(']') {
                return if after.trim().is_empty() {
                    Ok(Value::Strings(items))
                } else {
                    Err("配列の後に余分な文字があります")
                };
            }
            if !items.is_empty() {
                rest = rest
                    .strip_prefix(',')
                    .ok_or("配列の要素は , で区切ります")?
                    .trim_start();
            }
            let (item, after) = parse_string(rest)?;
            items.push(item);
            rest = after;
        }
    }
    if !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()) {
        return text
            .parse()
            .map(Value::Integer)
            .map_err(|_| "整数が範囲外です");
    }
    Err("値は \"文字列\"、整数、true/false、[\"文字列\", ...] のいずれかです")
}

/// 先頭の `"..."` を解析し、値と残りを返す。エスケープは `\"` と `\\` のみ。
fn parse_string(text: &str) -> Result<(String, &str), &'static str> {
    let body = text.strip_prefix('"').ok_or("文字列は \" で始めます")?;
    let mut value = String::new();
    let mut chars = body.char_indices();
    while let Some((index, c)) = chars.next() {
        match c {
            '"' => return Ok((value, &body[index + 1..])),
            '\\' => match chars.next() {
                Some((_, escaped @ ('"' | '\\'))) => value.push(escaped),
                _ => return Err("使えるエスケープは \\\" と \\\\ のみです"),
            },
            c if c.is_control() => return Err("文字列に制御文字があります"),
            c => value.push(c),
        }
    }
    Err("文字列が \" で閉じていません")
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
# 例
[daemon]
port = "COM7"
rotate_s = 5

[plugin clock]
command = ["C:\\stackchan\\clock.exe"]
rev = "0123456789ABCDEF0123456789abcdef01234567"
cards = 1
param.utc_offset_minutes = "540"

[plugin ai-usage]
command = [ "podman", "run", "--rm", "-i", "ai-usage" ]
cards = 2
notify = true
presence = true
motion = false
"#;

    #[test]
    fn example_config_is_parsed_with_secrets_merged() {
        let config = parse(
            EXAMPLE,
            Some("[plugin ai-usage]\nparam.token = \"s\\\"ecret\"\n"),
        )
        .unwrap();
        assert_eq!(
            config.daemon,
            DaemonConfig {
                port: Some("COM7".into()),
                rotate_s: 5
            }
        );
        let clock = &config.plugins[0];
        assert_eq!(clock.command, ["C:\\stackchan\\clock.exe"]);
        assert_eq!(
            clock.rev.as_deref(),
            Some("0123456789abcdef0123456789abcdef01234567")
        );
        assert_eq!(clock.allowed.cards, 1);
        assert_eq!(clock.params, [("utc_offset_minutes".into(), "540".into())]);
        let usage = &config.plugins[1];
        assert_eq!(usage.command.len(), 5);
        assert!(usage.allowed.notify && usage.allowed.presence && !usage.allowed.motion);
        assert_eq!(usage.params, [("token".into(), "s\"ecret".into())]);
    }

    #[test]
    fn summary_and_debug_do_not_show_secret_values() {
        let config = parse(
            "[plugin p]\ncommand = [\"x\"]\nparam.host = \"printer.local\"\nenv.HTTP_PROXY = \"http://user:pw@proxy\"\n",
            Some("[plugin p]\nparam.access_code = \"s3cr3t\"\n"),
        )
        .unwrap();
        let text = format!(
            "{}\n{:?}",
            summary(Path::new("/config"), &config),
            config.plugins
        );
        for secret in ["s3cr3t", "printer.local", "user:pw"] {
            assert!(!text.contains(secret), "{secret}: {text}");
        }
        for name in ["access_code", "host", "HTTP_PROXY"] {
            assert!(text.contains(name), "{name}: {text}");
        }
    }

    #[test]
    fn env_names_allow_uppercase_only_after_prefix() {
        let base = "[plugin p]\ncommand = [\"x\"]\n";
        let config = parse(
            &format!("{base}env.HTTP_PROXY = \"v\"\nenv.no_proxy = \"w\"\n"),
            None,
        )
        .unwrap();
        assert_eq!(config.plugins[0].env.len(), 2);
        for bad in [
            "env.1X = \"v\"",
            "env. = \"v\"",
            "Param.x = \"v\"",
            "env.A-B = \"v\"",
        ] {
            assert!(parse(&format!("{base}{bad}\n"), None).is_err(), "{bad}");
        }
        // secrets.conf には env を書けない (環境変数はプロセスの情報から見え得るため)。
        assert!(parse(base, Some("[plugin p]\nenv.TOKEN = \"v\"\n")).is_err());
    }

    #[test]
    fn defaults_apply_to_empty_config() {
        let config = parse("", None).unwrap();
        assert_eq!(config.daemon, DaemonConfig::default());
        assert!(config.plugins.is_empty());
    }

    #[test]
    fn errors_report_file_and_line() {
        let cases = [
            ("[daemon]\nport = COM7\n", 2),
            ("[daemon]\nunknown = 1\n", 2),
            ("[daemon]\nrotate_s = 0\n", 2),
            ("[daemon]\nport = \"a\"\nport = \"b\"\n", 3),
            ("[daemon]\n[daemon]\n", 2),
            ("key = \"x\"\n", 1),
            ("[plugin Clock]\n", 1),
            ("[plugin a]\ncommand = []\n", 2),
            ("[plugin a]\ncards = 9\n", 2),
            ("[plugin a]\nrev = \"abc\"\n", 2),
            (
                "[plugin a]\ncommand = [\"x\"]\n[plugin a]\ncommand = [\"x\"]\n",
                3,
            ),
            ("[plugin a]\nnotify = true\n", 1),
            ("[plugin a]\ncommand = [\"x\" \"y\"]\n", 2),
            ("[plugin a]\ncommand = [\"x\"] extra\n", 2),
            ("[plugin a]\nparam.k = \"tab\there\"\n", 2),
            ("[plugin a]\nparam.k = \"\\n\"\n", 2),
            ("[other]\n", 1),
        ];
        for (text, line) in cases {
            let error = parse(text, None).unwrap_err();
            assert_eq!(
                (error.file.as_str(), error.line),
                (CONFIG_FILE, line),
                "{text}"
            );
        }
    }

    #[test]
    fn secrets_cannot_add_plugins_or_permissions() {
        let base = "[plugin a]\ncommand = [\"x\"]\nparam.k = \"1\"\n";
        for secrets in [
            "[plugin b]\nparam.k = \"1\"\n",
            "[plugin a]\nmotion = true\n",
            "[plugin a]\nparam.k = \"2\"\n",
            "[daemon]\n",
        ] {
            let error = parse(base, Some(secrets)).unwrap_err();
            assert_eq!(error.file, SECRETS_FILE, "{secrets}");
        }
    }

    #[test]
    fn default_dir_follows_platform_conventions() {
        fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> + use<> {
            let pairs: Vec<(String, String)> = pairs
                .iter()
                .map(|(key, value)| ((*key).into(), (*value).into()))
                .collect();
            move |name| {
                pairs
                    .iter()
                    .find(|(key, _)| key == name)
                    .map(|(_, value)| OsString::from(value))
            }
        }
        // 絶対 path の判定は実行する OS の規則に従うため、試験値も OS ごとに用意する。
        #[cfg(windows)]
        let (appdata, xdg, home) = (r"C:\Users\u\AppData\Roaming", r"C:\xdg", r"C:\home");
        #[cfg(not(windows))]
        let (appdata, xdg, home) = ("/appdata", "/xdg", "/home/u");
        assert_eq!(
            default_dir(true, env(&[("APPDATA", appdata)])).unwrap(),
            Path::new(appdata).join("stackchan")
        );
        assert_eq!(
            default_dir(false, env(&[("XDG_CONFIG_HOME", xdg), ("HOME", home)])).unwrap(),
            Path::new(xdg).join("stackchan")
        );
        assert_eq!(
            default_dir(
                false,
                env(&[("XDG_CONFIG_HOME", "relative"), ("HOME", home)])
            )
            .unwrap(),
            Path::new(home).join(".config").join("stackchan")
        );
        assert!(default_dir(true, env(&[])).is_err());
        assert!(default_dir(false, env(&[("HOME", "relative")])).is_err());
    }

    /// 任意の入力で panic しないことを固定 seed の変異入力で確認する。
    #[test]
    fn parser_does_not_panic_on_mutated_input() {
        let mut state = 0x51F2_C0DE_77A1_9B03_u64;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state
        };
        let alphabet = b"[]=\"\\,# \n\tabcz09_.-";
        for _ in 0..3000 {
            let mut bytes = EXAMPLE.as_bytes().to_vec();
            for _ in 0..=(next() % 8) {
                let index = (next() as usize) % (bytes.len() + 1);
                match next() % 3 {
                    0 if index < bytes.len() => {
                        bytes.remove(index);
                    }
                    1 => bytes.insert(index, alphabet[(next() as usize) % alphabet.len()]),
                    _ => bytes.insert(index, next() as u8),
                }
            }
            let text = String::from_utf8_lossy(&bytes);
            let _ = parse(&text, Some(&text));
        }
    }
}
