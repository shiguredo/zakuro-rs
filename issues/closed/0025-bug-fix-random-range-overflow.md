# random_range 関数のオーバーフローとゼロ除算を修正する

- Priority: High
- Created: 2026-06-28
- Completed: 2026-06-28
- Model: DeepSeek V4 Pro
- Branch: feature/fix-random-range-overflow
- Polished: 2026-06-28

## 目的

`src/scenario.rs` の `random_range` 関数に存在する整数オーバーフローとゼロ除算の潜在バグを修正する。

## 優先度根拠

- `max < min` の場合に `u64` underflow による wrapping で `range=0` になり、後続の `% range` がゼロ除算 panic を引き起こす
- `max=u64::MAX, min=0` の場合も `u64::MAX - 0 + 1` が wrapping overflow で `range=0` になる
- 現在の呼び出し元 (`min_ms=1000, max_ms=5000`) では安全だが、同一クレート内の将来の呼び出しに対して防御的でない

## 現状

```rust
// src/scenario.rs:69-74
fn random_range(min: u64, max: u64) -> u64 {
    let range = max - min + 1;  // max < min で underflow、max=u64::MAX/min=0 で overflow
    let mut buf = [0u8; 8];
    aws_lc_rs::rand::fill(&mut buf).expect("random fill failed");
    min + u64::from_ne_bytes(buf) % range  // range=0 でゼロ除算 panic
}
```

## 設計方針

`assert!` で `max < min` を防止し、`range` 計算に `u128` を使用してオーバーフローを回避する。

`max - min + 1` が `u64::MAX + 1`（`max=u64::MAX, min=0` のケース）になる場合、`u64` では表現できない。`u128` にキャストして中間計算すればオーバーフローせず、正しい範囲が得られる。これにより `range=0` のゼロ除算 panic も完全に防止される。

## 完了条件

### 動作条件

- `max < min` のケースで `assert!` によりパニックすること（不正な呼び出しを早期検出）
- `max >= min` の全ケースで `range > 0` が保証され、ゼロ除算が発生しないこと
- 既存の呼び出し元 (`min_ms=1000, max_ms=5000`) で動作が変わらないこと

### テスト条件

- `tests/test_scenario.rs` を新規作成し、`random_range` 関数の単体テストを追加すること
- テストケース: `(max < min)` で `assert!` が発動すること、`(min=1000, max=5000)` で戻り値が範囲内であること、`(min=0, max=u64::MAX)` で `range > 0` が成立しパニックしないこと、`(min=max)` で範囲内の値が返ること
- テストのログメッセージは日本語にすること (CLAUDE.md 準拠)

## 解決方法

issue の設計方針どおり、`random_range` 関数を修正した。

- `assert!(max >= min, ...)` で不正な引数を早期検出
- `range` 計算を `u128` で行い、`max=u64::MAX, min=0` のケースでも `u64::MAX+1` がオーバーフローしないようにした

### 変更ファイル

- `src/scenario.rs`: `random_range` 関数のオーバーフロー修正

### テスト追加

- `test_random_range_max_less_than_min_panics`: `max < min` で assert 発動 (should_panic)
- `test_random_range_existing_range`: 既存呼び出し元 (1000-5000) で 1000 回の範囲内検証
- `test_random_range_u64_max_boundary`: `(0, u64::MAX)` でパニックしないことの検証
- `test_random_range_min_equals_max`: `min == max` で常に同一値が返ることの検証
