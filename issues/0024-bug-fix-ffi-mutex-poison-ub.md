# FFI 境界越え Mutex poison による未定義動作を修正する

- Priority: High
- Created: 2026-06-28
- Completed: 2026-00-00
- Model: DeepSeek V4 Pro
- Branch: feature/fix-ffi-mutex-poison-ub
- Polished: 2026-00-00

## 目的

`fake_audio_capturer.rs` の `register_audio_callback` と `virtual_client.rs` の `on_signaling_message` クロージャ内で Mutex の `.unwrap()` / `.expect()` が使用されており、Rust のパニックが FFI 境界を越えて未定義動作 (UB) を引き起こす可能性がある問題を修正する。

## 優先度根拠

未定義動作はプログラム全体の安全性を破壊し、クラッシュ・データ破損・セキュリティ脆弱性に繋がる。FFI 境界越えのパニックは特に深刻で、libwebrtc (C++) 側のスタックやリソース管理が破壊される。最優先で修正が必要。

## 現状

### fake_audio_capturer.rs:89

```rust
fn register_audio_callback(&self, transport: Option<AudioTransportRef>) -> i32 {
    let mut stored = self.audio_transport.lock().unwrap(); // ← poison で panic → FFI 越え UB
```

`register_audio_callback` は戻り値型が `i32` で C ABI から呼ばれることを意図している。audio_thread (`:256`) も同 Mutex の `.unwrap()` を使っており、audio_thread がパニックすると Mutex が poison され、次回の `register_audio_callback` でパニックが FFI 境界を越える。

### virtual_client.rs:421

```rust
*ids_for_sig.lock().expect("connection_ids mutex poisoned") = Some(parsed);
```

`on_signaling_message` クロージャは sora_sdk 経由で libwebrtc のシグナリングスレッドから呼ばれる可能性がある。同 Mutex は `run_stats_collection` (`:326`) からもロックされ、poison 時に FFI 越えパニックの危険。

## 設計方針

既に `video_device_capturer.rs:164` が採用している `let Ok(...) ... else { return; }` パターンを両箇所に適用する。

- `register_audio_callback`: poison 時は `return -1` でエラーを示す
- `on_signaling_message`: poison 時はエラーログを出して早期 return する

## 完了条件

- `fake_audio_capturer.rs:89` の `.unwrap()` が安全なエラーハンドリングに置き換えられていること
- `fake_audio_capturer.rs:256` の `.unwrap()` が安全なエラーハンドリングに置き換えられていること
- `virtual_client.rs:421` の `.expect()` が安全なエラーハンドリングに置き換えられていること
- `virtual_client.rs:326` の `.expect()` も同様に対応されていること

## 解決方法

### fake_audio_capturer.rs

```rust
// line 89:
let Ok(mut stored) = self.audio_transport.lock() else { return -1; };

// line 256:
let Ok(stored_ref) = state.audio_transport.lock() else { continue; };
```

### virtual_client.rs

```rust
// line 421:
if let Ok(mut ids) = ids_for_sig.lock() {
    *ids = Some(parsed);
}

// line 326:
let Ok(parsed) = ids.lock() else { continue; };
```
