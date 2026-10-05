//! plugin ごとのコンテナ分離で、daemon・起動補助 (`plugin-run`)・起動スクリプトの間で
//! 受け渡す情報。設計は docs/plugin.md の「plugin ごとのコンテナ分離」を参照する。

use std::ffi::OsString;

use crate::config::Config;

#[cfg(unix)]
pub use wire::{Launch, receive, send};

/// 起動情報の受渡し。Unix socket での接続にだけ用いる。
#[cfg(unix)]
mod wire {
    use std::io::{self, Read, Write};

    use plugin_api::{FrameError, FrameReader};

    use crate::config::PluginConfig;

    /// daemon が接続の直後に `plugin-run` へ送る起動情報。argv と、追加の環境変数。
    ///
    /// plugin のコンテナに設定ディレクトリを渡さずに argv を伝えるために送る。plugin の
    /// 作者が扱うものではないため `plugin-api` には置かない。derive を要しない組で表し、
    /// host の crate に serde への直接の依存を増やさない。
    pub type Launch = (Vec<String>, Vec<(String, String)>);

    pub fn send(writer: &mut impl Write, config: &PluginConfig) -> Result<(), FrameError> {
        let launch: Launch = (config.command.clone(), config.env.clone());
        plugin_api::write_frame(writer, &launch)
    }

    /// 起動情報の 1 フレームを読む。
    ///
    /// 区切りまでを 1 byte ずつ読む。この後 reader は plugin の stdin として渡すため、
    /// 起動情報より後ろの byte を先読みして失わないようにする。
    pub fn receive(reader: &mut impl Read) -> Result<Launch, FrameError> {
        let mut frame = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            match reader.read(&mut byte) {
                Ok(0) => return Err(plugin_api::FrameError::Truncated),
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error.into()),
            }
            frame.push(byte[0]);
            if byte[0] == 0 {
                break;
            }
            if frame.len() >= plugin_api::MAX_FRAME_BYTES {
                return Err(plugin_api::FrameError::Oversized);
            }
        }
        let launch: Launch = FrameReader::new(frame.as_slice())
            .read_frame()?
            .ok_or(FrameError::Truncated)?;
        if launch.0.is_empty() || launch.0[0].is_empty() {
            return Err(plugin_api::FrameError::Malformed);
        }
        Ok(launch)
    }
}

/// plugin に引き継ぐ起動元の環境変数。起動元の環境 (利用者のシェルにあるトークン等) を
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

/// plugin の環境変数。許可した起動元の変数と、利用者設定の `env.*` だけからなる。
/// 起動元は、子プロセス方式では daemon、コンテナ分離では `plugin-run` である。
pub fn child_env(
    configured: &[(String, String)],
    lookup: impl Fn(&str) -> Option<OsString>,
) -> Vec<(String, OsString)> {
    let mut env: Vec<(String, OsString)> = INHERITED_ENV
        .iter()
        .filter_map(|name| lookup(name).map(|value| ((*name).to_owned(), value)))
        .collect();
    for (name, value) in configured {
        env.retain(|(existing, _)| existing != name);
        env.push((name.clone(), value.into()));
    }
    env
}

/// 起動スクリプトがコンテナを起動するために必要な項目。1 行 1 件のタブ区切り。
///
/// 設定の構文の解釈をシェル側に重複させないために出力する。argv、env、param は
/// 出力しない。`dir` は任意の文字を含み得るため行の最後に置く (タブと改行は設定の
/// 検証で拒否している)。
pub fn plan(config: &Config) -> String {
    let mut lines = String::new();
    for plugin in &config.plugins {
        lines.push_str(&format!(
            "plugin\t{}\t{}\n",
            plugin.id,
            plugin.dir.as_deref().unwrap_or("")
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(text: &str) -> Config {
        crate::config::parse(text, None).unwrap()
    }

    #[test]
    #[cfg(unix)]
    fn launch_round_trips_and_leaves_following_bytes_unread() {
        let config = config(
            "[plugin p]\ncommand = [\"/plugin/bin\", \"\", \"--flag\"]\nenv.NAME = \"value\"\n",
        );
        let mut bytes = Vec::new();
        send(&mut bytes, &config.plugins[0]).unwrap();
        bytes.extend_from_slice(b"following");
        let mut reader = bytes.as_slice();
        let (argv, env) = receive(&mut reader).unwrap();
        assert_eq!(argv, ["/plugin/bin", "", "--flag"]);
        assert_eq!(env, [("NAME".to_owned(), "value".to_owned())]);
        assert_eq!(reader, b"following");
    }

    #[test]
    #[cfg(unix)]
    fn launch_rejects_closed_empty_and_oversized_input() {
        assert!(matches!(
            receive(&mut [].as_slice()),
            Err(plugin_api::FrameError::Truncated)
        ));
        assert!(matches!(
            receive(&mut [1u8, 2, 3].as_slice()),
            Err(plugin_api::FrameError::Truncated)
        ));
        let mut empty_argv = Vec::new();
        let launch: Launch = (Vec::new(), Vec::new());
        plugin_api::write_frame(&mut empty_argv, &launch).unwrap();
        assert!(matches!(
            receive(&mut empty_argv.as_slice()),
            Err(plugin_api::FrameError::Malformed)
        ));
        let endless = vec![1u8; plugin_api::MAX_FRAME_BYTES + 1];
        assert!(matches!(
            receive(&mut endless.as_slice()),
            Err(plugin_api::FrameError::Oversized)
        ));
    }

    #[test]
    fn plugins_receive_only_allowed_and_configured_environment() {
        let config = config(
            "[plugin p]\ncommand = [\"x\"]\nenv.HTTP_PROXY = \"http://proxy:8080\"\nenv.PATH = \"/opt/bin\"\n",
        );
        let parent_env = |name: &str| match name {
            "PATH" => Some("/usr/bin".into()),
            "HOME" => Some("/root".into()),
            "GH_TOKEN" | "ANTHROPIC_API_KEY" => Some("secret".into()),
            _ => None,
        };
        let env = child_env(&config.plugins[0].env, parent_env);
        let names: Vec<&str> = env.iter().map(|(name, _)| name.as_str()).collect();
        assert_eq!(names, ["HOME", "HTTP_PROXY", "PATH"]);
        assert!(
            env.contains(&("PATH".into(), "/opt/bin".into())),
            "設定が優先する"
        );
        assert!(env.iter().all(|(_, value)| value != "secret"));
    }

    #[test]
    fn plan_lists_plugins_without_arguments_or_parameters() {
        let config = crate::config::parse(
            "[plugin clock]\ncommand = [\"/plugin/clock\", \"--secret-arg\"]\n\
             [plugin ext]\ncommand = [\"/plugin/ext\"]\ndir = \"C:/Users/a b/release\"\n\
             param.token = \"hidden\"\nenv.E = \"hidden\"\n",
            None,
        )
        .unwrap();
        assert_eq!(
            plan(&config),
            "plugin\tclock\t\nplugin\text\tC:/Users/a b/release\n"
        );
    }
}
