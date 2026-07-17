//! dep-direction-check.sh セルフテスト用 fixture（TASK-11.1、#33）。
//!
//! チェック 2 の正例: `src/lib.rs` が無く `src/main.rs` のみを持つバイナリクレートで、
//! `src/lib.rs` → `src/main.rs` のフォールバック解決が機能し、かつ宣言（依存方向:
//! server → routes → http::*）を検出できることを確認する fixture。
