# random_range 関数のオーバーフローとゼロ除算を修正する

- Priority: High
- Created: 2026-06-28
- Completed: 2026-00-00
- Model: DeepSeek V4 Pro
- Branch: feature/fix-random-range-overflow
- Polished: 2026-00-00

## 目的

`src/scenario.rs` の `random_range` 関数に存在する整数オーバーフローとゼロ除算の静的バグを修正する。

## 優先度根拠

- `max < min` の場合に `u64` underflow による wrapping で `range=0` になり、後続の `% range` がゼロ除算 panic を引き起こす
- `max=u64::MAX, min=0` の場合も `u64::MAX - 0 + 1` が wrapping overflow で `range=0` になる
- 現在の呼び出し元 (`min_ms=1000, max_ms=5000`) では安全だが、`pub(crate)` 関数として同一クレート内の将来の呼び出しで容易に panic を引き起こす潜在バグ

## 現状

```rust
// src/scenario.rs:69-73
fn random_range(min: u64, max: u64) -> u64 {
    let range = max - min + 1;  // max < min で underflow、max=u64::MAX/min=0 で overflow
    let mut buf = [0u8; 8];
    aws_lc_rs::rand::fill(&mut buf).expect("random fill failed");
    min + u64::from_ne_bytes(buf) % range  // range=0 でゼロ除算 panic
}
```

## 設計方針

1. `max < min` の場合は min と max を入れ替えるか、エラーを返す
2. `range` の計算に `checked_sub` と `checked_add` を使用してオーバーフローを検出する
3. あるいは呼び出し元での使用パターンが `min_ms=1000, max_ms=5000` 固定なので、`pub(crate)` を外して `fn` にスコープを狭めることも検討する

## 完了条件

- `max < min` のケースでパニックしないこと（エラーハンドリングされていること）
- `range=0` でゼロ除算 panic が発生しないこと
- 既存の呼び出し元 (`min_ms=1000, max_ms=5000`) で動作が変わらないこと

## 解決方法

`max < min` のチェックを追加し、`checked_sub` で安全に計算する:

```rust
fn random_range(min: u64, max: u64) -> u64 {
    assert!(max >= min, "random_range: max must be >= min");
    let range = max.checked_sub(min).unwrap_or(0).checked_add(1).unwrap_or(1);
    let mut buf = [0u8; 8];
    aws_lc_rs::rand::fill(&mut buf).expect("random fill failed");
    min + u64::from_ne_bytes(buf) % range
}
```
