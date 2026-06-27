# FFI 境界越え Mutex poison による未定義動作を修正する

- Priority: High
- Created: 2026-06-28
- Completed: 2026-00-00
- Model: DeepSeek V4 Pro
- Branch: feature/fix-ffi-mutex-poison-ub
- Polished: 2026-06-28

## 目的

`fake_audio_capturer.rs` および `virtual_client.rs` 内の Mutex `.unwrap()` / `.expect()` を安全なエラーハンドリングに置き換える。Rust のパニックが FFI 境界を越えて未定義動作 (UB) を引き起こすリスクを除去する。対象は FFI 境界上にある呼び出し (`register_audio_callback`, `on_signaling_message`) と、それらと Mutex を共有する通常スレッド (`audio_thread`, `run_stats_collection`) の計 4 箇所。

## 優先度根拠

FFI 境界越えのパニックは libwebrtc (C++) 側のスタックやリソース管理を破壊し、最優先で修正が必要。

## 現状

### 問題の構造

以下の 4 箇所が 2 つの Mutex を共有しており、一方のパニックが Mutex poison を経由して他方の FFI 境界越えパニックに連鎖する:

| 箇所 | ファイル:行 | コンテキスト | FFI 境界越え |
|------|-------------|-------------|-------------|
| `register_audio_callback` | `fake_audio_capturer.rs:89` | C ABI コールバック (libwebrtc) | **あり** |
| `audio_thread` | `fake_audio_capturer.rs:256` | Rust スレッド | なし (同 Mutex を共有 → poison 時に FFI 側へ連鎖) |
| `on_signaling_message` | `virtual_client.rs:421` | libwebrtc シグナリング コールバック | **あり** |
| `run_stats_collection` | `virtual_client.rs:326` | tokio タスク | なし (同 Mutex を共有 → poison 時に FFI 側へ連鎖) |

### 共有 Mutex

- `fake_audio_capturer.rs` の `audio_transport: Arc<Mutex<Option<AudioTransportRef>>>` を `register_audio_callback` (:89) と `audio_thread` (:256) が共有
- `virtual_client.rs` の `ids: Arc<Mutex<Option<ConnectionIds>>>` を `on_signaling_message` (:421) と `run_stats_collection` (:326) が共有

### 設計上の経緯

`virtual_client.rs:421` の `.expect("connection_ids mutex poisoned")` は closed 0005 (DuckDB 統計書き込み追加) で意図的に採用された。当時は FFI 越えリスクが考慮されておらず、今回新たに安全性の観点から修正する。

## 設計方針

`video_device_capturer.rs:164` で既に採用されている `let Ok(...) = ... else { return; }` パターンをベースに、以下を全 4 箇所に適用する:

| 箇所 | コンテキスト | poison 時動作 | ログ |
|------|-------------|--------------|------|
| `register_audio_callback` | FFI C ABI | `return -1` (libwebrtc ADM 規約: 負値はエラー) | あり |
| `audio_thread` | Rust スレッド | `continue` (当該 10ms フレームのみ欠落) | あり |
| `on_signaling_message` | FFI コールバック | early `return` | あり |
| `run_stats_collection` | tokio タスク | `continue` | あり |

- ログは `rtc_log_warning!` を使用 (CLAUDE.md 準拠)
- ロック保持区間は元コードと同じスコープを維持する (意図しない延長を防止)

### `on_signaling_message` の `try_send` 順序問題

現在のコードは `try_send(InsertConnection(...))` → `lock()` の順で実行され、poison 時に DuckDB 書込成功・ids 未更新の不整合が生じる。修正では `try_send` を lock 成功後に移動し、poison 時は両方実行されないようにする。ただし `try_send` 自体の failure (channel full) は本 issue の対象外とする（channel full 時は `duckdb_stats.rs` 側でドロップカウントが記録される）。

### 制約

- `std::sync::Mutex::lock()` はブロッキング呼び出しであり、tokio コンテキストでの使用は協調的マルチタスクの原則に反するが、ロック保持時間が極短いため本 issue では修正範囲外とする
- `std::sync::Mutex` の poison は Rust の仕様上解除不可能であり、一度 poison された後は常に `Err` を返す。`let Ok(...) else { ... }` パターンはパニックは防止するが poison 自体は消えない

## 完了条件

### 動作条件

- 全 4 箇所で `.unwrap()` / `.expect()` が安全なエラーハンドリングに置き換えられていること
- `register_audio_callback` (:89) が poison 時に `-1` を返すこと
- `on_signaling_message` (:421) で `try_send` が lock 成功後に実行されること
- 非 poison 時の挙動（正常系）が一切変更されていないこと

### テスト条件

- `tests/test_fake_audio_capturer.rs` に、poison 済み Mutex を意図的に作成し `register_audio_callback` が `-1` を返すことを検証するテストを追加すること
- `tests/test_virtual_client.rs` に、`on_signaling_message` の `try_send` 順序変更後の動作を検証するテストを追加すること
- テストのログメッセージは日本語にすること (CLAUDE.md 準拠)
- 既存のテストがすべて通過すること

## 解決方法

### fake_audio_capturer.rs

`use` 宣言に `rtc_log_warning` を追加:

```rust
use shiguredo_webrtc::{AudioDeviceModule, AudioDeviceModuleHandler, AudioTransportRef, rtc_log_warning};
```

`register_audio_callback` (:89):

```rust
let Ok(mut stored) = self.audio_transport.lock() else {
    rtc_log_warning!("audio_transport mutex poisoned in register_audio_callback");
    return -1;
};
```

`audio_thread` (:256) — 元コードのブロックスコープを維持し、ロック保持期間を延長させない:

```rust
let transport = {
    let Ok(guard) = state.audio_transport.lock() else {
        rtc_log_warning!("audio_transport mutex poisoned in audio_thread");
        continue;
    };
    *guard
}; // guard はここで drop → ロック解放済み → recorded_data_is_available は非ロック状態で呼ばれる
```

`continue` による `next_time` 更新スキップについては、当該 10ms フレーム 1 回分の欠落に留まるため許容範囲とする。

### virtual_client.rs

`on_signaling_message` (:399-422) — `try_send` を lock 成功後に移動:

```rust
builder = builder.on_signaling_message(move |_type_, direction, text| {
    if direction != SignalingDirection::Received {
        return;
    }
    let Some(parsed) = parse_offer_ids(text) else {
        return;
    };
    let Ok(mut guard) = ids_for_sig.lock() else {
        rtc_log_warning!(
            "[i{}/vc-{}] connection_ids mutex poisoned in on_signaling_message",
            instance_id,
            vc_id,
        );
        return;
    };
    duckdb_for_sig.try_send(WriteCommand::InsertConnection(Box::new(
        InsertConnectionRow {
            instance_id,
            vc_id,
            timestamp: SystemTime::now(),
            channel_id: channel_id_for_sig.clone(),
            connection_id: parsed.connection_id.clone(),
            session_id: parsed.session_id.clone(),
            role: role_str.clone(),
            audio: audio_value,
            video: video_value,
        },
    )));
    *guard = Some(parsed);
});
```

`run_stats_collection` (:324-326) — ブロックスコープで guard を即 drop し、await 越しの保持を防止:

```rust
let Some(parsed) = {
    let Ok(guard) = ids.lock() else {
        rtc_log_warning!(
            "[i{}/vc-{}][duckdb] connection_ids mutex poisoned in stats_collection",
            instance_id,
            vc_id,
        );
        continue;
    };
    guard.clone()
} else {
    skipped_iters += 1;
    continue;
};
// guard はここで既に drop 済み → 後続の handle.get_stats().await は非ロック状態
```
