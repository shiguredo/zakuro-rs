//! `zakuro lint` サブコマンド (JSONC 設定の妥当性検証)

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use nojson::RawJson;

use crate::args;
use crate::diagnostic::{emit_error_message, emit_error_with_span};
use crate::error::{AppError, ErrorMessage, Result};

/// `lint` サブコマンドを処理する
///
/// - `Ok(Some(code))`: lint を実行した (呼び出し元は当該 ExitCode で終了する)
/// - `Ok(None)`: lint サブコマンドではない (通常の負荷試験起動へ進む)
pub(crate) fn try_run() -> Result<Option<ExitCode>> {
    // 第一版は `zakuro lint <FILE>` のみ (サブコマンドは argv[1])
    let mut env_iter = std::env::args();
    let _program = env_iter.next();
    if env_iter.next().as_deref() != Some("lint") {
        return Ok(None);
    }

    let mut args = noargs::raw_args();
    args.metadata_mut().app_name = env!("CARGO_PKG_NAME");
    args.metadata_mut().app_description = "Lint zakuro JSONC configuration file";

    if !noargs::cmd("lint")
        .doc("Lint a zakuro JSONC configuration file")
        .take(&mut args)
        .is_present()
    {
        // argv[1] が lint なのに take できなければ実装バグ
        return Err(ErrorMessage::new("internal error: failed to take lint subcommand").into());
    }

    noargs::HELP_FLAG.take_help(&mut args);

    let path: PathBuf = noargs::arg("<FILE>")
        .doc("JSONC configuration file to lint")
        .example("config.jsonc")
        .take(&mut args)
        .then(|a| a.value().parse())?;

    if let Some(help) = args.finish()? {
        print!("{help}");
        return Ok(Some(ExitCode::SUCCESS));
    }

    match lint_file(&path) {
        Ok(()) => Ok(Some(ExitCode::SUCCESS)),
        Err(()) => Ok(Some(ExitCode::from(1))),
    }
}

/// 1 ファイルを lint する
///
/// 成功時は出力なし。失敗時は annotate-snippets 形式で
/// stderr に診断を出して `Err(())` を返す (呼び出し側で exit 1)。
fn lint_file(path: &Path) -> std::result::Result<(), ()> {
    let path_display = path.display().to_string();
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            emit_error_message(&format!("{path_display}: {e}"));
            return Err(());
        }
    };

    // 構文エラーは位置付きで報告する
    if let Err(e) = RawJson::parse_jsonc(&content) {
        let start = e.position();
        // 0 長 span だと下線が出ないため最小 1 バイトに揃える
        let span = start..start + 1;
        emit_error_with_span(&path_display, &content, &format!("{e}"), span);
        return Err(());
    }

    match args::validate_jsonc_config_str(&content) {
        Ok((_common, _instances)) => Ok(()),
        Err(err) => {
            emit_app_error(&path_display, &err);
            Err(())
        }
    }
}

fn emit_app_error(path: &str, err: &AppError) {
    match err {
        AppError::Message(msg) => {
            emit_error_message(&format!("{path}: {msg}"));
        }
        AppError::Args(e) => {
            // noargs::Error は Display 未実装のため Debug で出す
            emit_error_message(&format!("{path}: {e:?}"));
        }
        other => {
            emit_error_message(&format!("{path}: {other}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// 正しい最小 JSONC は lint に成功すること
    #[test]
    fn lint_file_accepts_minimal_valid_config() {
        let dir = tempfile::tempdir().expect("一時ディレクトリを作成できること");
        let path = dir.path().join("ok.jsonc");
        let mut file = std::fs::File::create(&path).expect("ファイルを作成できること");
        write!(
            file,
            r#"{{
  // 最小の接続設定
  "sora": {{
    "signaling-url": "ws://127.0.0.1:5000/signaling",
    "channel-id": "lint-test",
    "role": "sendonly"
  }}
}}
"#
        )
        .expect("書き込めること");
        assert!(
            lint_file(&path).is_ok(),
            "最小の正しい JSONC は lint に成功すること"
        );
    }

    /// 構文エラーは失敗すること
    #[test]
    fn lint_file_rejects_syntax_error() {
        let dir = tempfile::tempdir().expect("一時ディレクトリを作成できること");
        let path = dir.path().join("bad.jsonc");
        std::fs::write(&path, "{ // broken\n").expect("書き込めること");
        assert!(
            lint_file(&path).is_err(),
            "構文不正な JSONC は lint に失敗すること"
        );
    }

    /// 意味エラー (必須欠落) は失敗すること
    #[test]
    fn lint_file_rejects_missing_required_fields() {
        let dir = tempfile::tempdir().expect("一時ディレクトリを作成できること");
        let path = dir.path().join("incomplete.jsonc");
        // signaling-url / channel-id が無い
        std::fs::write(&path, "{\n  \"vcs\": 1\n}\n").expect("書き込めること");
        assert!(
            lint_file(&path).is_err(),
            "必須フィールド欠落は lint に失敗すること"
        );
    }
}
