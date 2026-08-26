//! `zakuro fmt` サブコマンド (JSONC 設定の整形)

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crate::diagnostic::{emit_error_message, emit_error_with_span};
use crate::error::{ErrorMessage, Result};
use crate::jsonc_fmt;

/// `fmt` サブコマンドを処理する
///
/// - `Ok(Some(code))`: fmt を実行した (呼び出し元は当該 ExitCode で終了する)
/// - `Ok(None)`: fmt サブコマンドではない (通常の負荷試験起動へ進む)
pub(crate) fn try_run() -> Result<Option<ExitCode>> {
    let mut env_iter = std::env::args();
    let _program = env_iter.next();
    if env_iter.next().as_deref() != Some("fmt") {
        return Ok(None);
    }

    let mut args = noargs::raw_args();
    args.metadata_mut().app_name = env!("CARGO_PKG_NAME");
    args.metadata_mut().app_description = "Format zakuro JSONC configuration file";

    if !noargs::cmd("fmt")
        .doc("Format a zakuro JSONC configuration file (2-space indent, comments preserved)")
        .take(&mut args)
        .is_present()
    {
        return Err(ErrorMessage::new("internal error: failed to take fmt subcommand").into());
    }

    noargs::HELP_FLAG.take_help(&mut args);

    let path: PathBuf = noargs::arg("<FILE>")
        .doc("JSONC configuration file to format")
        .example("config.jsonc")
        .take(&mut args)
        .then(|a| a.value().parse())?;

    if let Some(help) = args.finish()? {
        print!("{help}");
        return Ok(Some(ExitCode::SUCCESS));
    }

    match fmt_file(&path) {
        Ok(()) => Ok(Some(ExitCode::SUCCESS)),
        Err(()) => Ok(Some(ExitCode::from(1))),
    }
}

/// 1 ファイルを fmt する
///
/// 成功時は変更があれば書き戻し、無ければ無出力。失敗時は annotate-snippets 形式で
/// stderr に診断を出して `Err(())` を返す。
fn fmt_file(path: &Path) -> std::result::Result<(), ()> {
    let path_display = path.display().to_string();
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => {
            emit_error_message(&format!("{path_display}: {e}"));
            return Err(());
        }
    };

    let formatted = match jsonc_fmt::format_jsonc(&content) {
        Ok(s) => s,
        Err(e) => {
            let start = e.position();
            let span = start..start + 1;
            emit_error_with_span(&path_display, &content, &format!("{e}"), span);
            return Err(());
        }
    };

    if formatted == content {
        return Ok(());
    }

    if let Err(e) = std::fs::write(path, &formatted) {
        emit_error_message(&format!("{path_display}: {e}"));
        return Err(());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 整形でコメントと trailing comma が保持されること
    #[test]
    fn fmt_file_preserves_comments_and_trailing_comma() {
        let dir = tempfile::tempdir().expect("一時ディレクトリを作成できること");
        let path = dir.path().join("config.jsonc");
        let input = r#"{
"sora":{
"signaling-url":"ws://127.0.0.1:5000/signaling", // endpoint
"channel-id":"fmt-test",
"role":"sendonly",
},
}"#;
        std::fs::write(&path, input).expect("書き込めること");
        fmt_file(&path).expect("fmt に成功すること");

        let output = std::fs::read_to_string(&path).expect("読み込めること");
        assert!(output.contains("// endpoint"), "行コメントが保持されること");
        assert!(
            output.contains("\"role\": \"sendonly\","),
            "trailing comma が保持されること"
        );
        assert!(
            output.contains("\"signaling-url\": \"ws://127.0.0.1:5000/signaling\""),
            "値の空白が正規化されること"
        );
        assert!(
            output.contains("\n  \"sora\": {\n"),
            "2 スペースインデントになること"
        );
    }

    /// 既に整形済みのファイルは書き換えないこと
    #[test]
    fn fmt_file_skips_write_when_unchanged() {
        let dir = tempfile::tempdir().expect("一時ディレクトリを作成できること");
        let path = dir.path().join("same.jsonc");
        let content = "{\n  \"vcs\": 1\n}\n";
        std::fs::write(&path, content).expect("書き込めること");
        let before = std::fs::metadata(&path)
            .expect("メタデータを取得できること")
            .modified()
            .expect("更新時刻を取得できること");
        fmt_file(&path).expect("fmt に成功すること");
        let after = std::fs::metadata(&path)
            .expect("メタデータを取得できること")
            .modified()
            .expect("更新時刻を取得できること");
        assert_eq!(before, after, "変更が無いときは書き込まないこと");
    }

    /// 構文エラーは失敗すること
    #[test]
    fn fmt_file_rejects_syntax_error() {
        let dir = tempfile::tempdir().expect("一時ディレクトリを作成できること");
        let path = dir.path().join("bad.jsonc");
        std::fs::write(&path, "{ broken").expect("書き込めること");
        assert!(
            fmt_file(&path).is_err(),
            "構文不正な JSONC は fmt に失敗すること"
        );
    }
}
