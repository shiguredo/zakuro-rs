//! ビルド時に実行ファイルへ共有ライブラリの探索パス (rpath) を埋め込む。
//!
//! DuckDB は prebuilt の共有ライブラリとしてリンクされる。libduckdb-sys の
//! build script も rpath を発行しているが、依存クレートの build script が出す
//! `cargo:rustc-link-arg` は zakuro のバイナリへ伝播しない。そのため zakuro
//! 自身の build script で実行ファイル相対の rpath を埋め込む。

fn main() {
    // Windows は rpath の概念が無く、DLL は実行ファイルと同じディレクトリへ
    // 置く必要があるため対象外とする。
    let target_os =
        std::env::var("CARGO_CFG_TARGET_OS").expect("CARGO_CFG_TARGET_OS should be set by cargo");
    let rpath_prefix = match target_os.as_str() {
        "macos" => "@executable_path",
        "linux" => "$ORIGIN",
        _ => return,
    };

    // 実行ファイルと同じディレクトリと、libduckdb-sys が共有ライブラリを
    // コピーする `deps` の両方を探索パスにする。前者は配布物で実行ファイルと
    // 同じディレクトリへ同梱する配置、後者はビルドツリーの配置に対応する。
    for suffix in ["", "/deps"] {
        println!("cargo:rustc-link-arg-bins=-Wl,-rpath,{rpath_prefix}{suffix}");
    }

    // libduckdb-sys は prebuilt DuckDB を展開したディレクトリを
    // `DEP_DUCKDB_INCLUDE` で公開する。`cargo run` やテストでは Cargo が
    // 探索パスを設定するが、ビルドしたバイナリを直接起動するときは設定されない
    // ため、そのディレクトリを rpath に追加する。
    println!("cargo:rerun-if-env-changed=DEP_DUCKDB_INCLUDE");
    if let Ok(dir) = std::env::var("DEP_DUCKDB_INCLUDE") {
        // ビルドツリーから直接起動する場合に libduckdb.so を見つけられるようにする。
        println!("cargo:rustc-link-arg-bins=-Wl,-rpath,{dir}");
    }
}
