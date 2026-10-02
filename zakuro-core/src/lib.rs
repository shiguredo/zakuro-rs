//! zakuro の負荷試験ツールで共有する基盤
//!
//! Sora (WebRTC) 版と MOQ 版のバイナリで共有する、libwebrtc に依存しない処理を置く。
//! ログは `log` クレートのファサードを使い、出力先と形式は各バイナリが決める
//! (Sora 版は libwebrtc のログ出力へ、MOQ 版は tracing へ転送する)。

pub mod http_server;
pub mod json_rpc;
pub mod stats;
