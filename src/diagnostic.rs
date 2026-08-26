//! CLI 診断メッセージの表示 (mikan lint と同系統の annotate-snippets 出力)

use std::io::IsTerminal;
use std::ops::Range;
use std::sync::OnceLock;

use annotate_snippets::renderer::DecorStyle;
use annotate_snippets::{AnnotationKind, Group, Level, Renderer, Snippet};

/// stderr 向け診断レンダラ (プロセスで 1 回だけ初期化)
///
/// stderr が TTY のとき色付き + Unicode 罫線、それ以外は ASCII・無色。
/// `annotate-snippets` の `Renderer::styled()` は無条件で ANSI を出すため、
/// パイプや CI ログへ ANSI / box-drawing が混入しないよう分岐する。
fn stderr_renderer() -> &'static Renderer {
    static STDERR_RENDERER: OnceLock<Renderer> = OnceLock::new();
    STDERR_RENDERER.get_or_init(|| {
        if std::io::stderr().is_terminal() {
            Renderer::styled().decor_style(DecorStyle::Unicode)
        } else {
            Renderer::plain().decor_style(DecorStyle::Ascii)
        }
    })
}

/// ソース位置付きのエラー診断を stderr に出力する
///
/// mikan の `emit_report` と同じく、タイトル + ソース注釈を 1 グループで描画する。
/// `span` が空のときは下線が出ないため、呼び出し側で長さ 1 以上にする。
pub(crate) fn emit_error_with_span(path: &str, source: &str, message: &str, span: Range<usize>) {
    let title = Level::ERROR.primary_title(message);
    let snippet = Snippet::source(source)
        .path(path)
        .annotation(AnnotationKind::Primary.span(span));
    let groups = [title.element(snippet)];
    eprintln!("{}", stderr_renderer().render(&groups));
}

/// ソース位置の無いエラー診断を stderr に出力する
///
/// mikan の `report_diagnostic_error` と同様、タイトルのみの Group を描画する。
pub(crate) fn emit_error_message(message: &str) {
    let groups = [Group::with_title(Level::ERROR.primary_title(message))];
    eprintln!("{}", stderr_renderer().render(&groups));
}
