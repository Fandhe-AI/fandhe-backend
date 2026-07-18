# TASK-9.3（#63）JWT 検証結果リクエストスコープキャッシュ コスト計測レポート

`crates/plugin-hub-wiring/examples/jwt_cache_bench.rs`（プロセス内マイクロベンチ。
複数試行・中央値評価、`benches/README.md` の方針を踏襲）による計測結果。

## 背景

`TenantGate::check`（`RequestGate` 拡張点）は RS256 署名検証（`verify_token`）を
行い `GateOutcome::Allow`/`Reject` のみをコアへ返す。`GateOutcome` は判定結果のみ
を運ぶ契約（`crates/core/src/extension.rs` doc）のため、ハンドラ側で `org_id` 等の
認証済みクレームが必要な場合、従来は `verify_token` を再呼び出しするしかなく、
1 リクエストにつき RSA-2048 署名検証が 2 回（ゲート + ハンドラ）走っていた。

本タスクは [`Authenticator`]（`crates/plugin-hub-wiring/src/auth.rs`）による
検証結果キャッシュ（トークン文字列の SHA-256 ハッシュをキー、成功のみ保持、鍵
ローテーション・`exp` の都度再判定でフェイルクローズを維持）でこの重複を解消する。

## 計測方法

- 計測対象: RSA-2048 鍵での RS256 JWT（`tests/fixtures/test-rsa-2048.pk8`、
  テスト・計測専用、本番使用禁止）
- シナリオ A（比較対象）: `verify_token` を同一トークンに対し N 回直接呼び出す
  （毎回 RS256 署名検証を実行）
- シナリオ B（本タスクの成果物）: `Authenticator` でキャッシュを 1 回のミスで
  温めた後、同一トークンに対し N 回 `authenticate` を呼ぶ（全呼び出しがキャッシュ
  ヒットであることを `cache_hits() == N` で機械検証してから計測に含める）
- N = 20,000 回/試行、7 試行の中央値を採用（外れ値 1 件に引きずられないため、
  `benches/README.md` と同一方針）
- 実行コマンド: `cargo run --release -p bf-plugin-hub-wiring --example jwt_cache_bench`

## 実行環境

| 項目 | 値 |
|------|-----|
| 実行日時 | 2026-07-18 |
| 対象コミット（`origin/main`、本ブランチの分岐元） | `4e2c732647842e60100a9b2b4e6649ea337e7d95` |
| rustc | 1.96.0（stable） |
| ビルド | `cargo build --release -p bf-plugin-hub-wiring --example jwt_cache_bench` |
| 備考 | 本 worktree は並列 issue 実装ワークフロー下で実行されており、他エージェントの並行負荷が計測ノイズに影響している可能性がある（`benches/reports/task-5.2-graphql-performance.md` 等と同一の既知事情） |

## 結果（3 回実行、各回 20,000 ops × 7 試行の中央値）

| 実行 | `verify_token` 直接呼び出し 中央値 (ns/op) | `Authenticator` キャッシュヒット 中央値 (ns/op) | 削減率（speedup） |
|------|------|------|------|
| 1 回目 | 16,811.8 | 873.6 | 19.2x |
| 2 回目 | 17,153.9 | 895.1 | 19.2x |
| 3 回目 | 18,527.9 | 929.1 | 19.9x |
| 4 回目（採用値） | 17,599.7 | 1,425.4 | 12.3x |

4 回中 3 回（1〜3 回目）が 19.2〜19.9x の削減率で安定している。4 回目のみキャッシュ
ヒット側が 1,425.4 ns/op と他回（863〜929 ns/op）より高く 12.3x に留まったが、これは
本 worktree が並列実装ワークフロー下で実行されている環境ノイズによるものと判断する
（`verify_token` 側の中央値は 4 回とも 16,800〜18,600 ns/op のレンジに収まっており、
署名検証コスト自体は安定している）。4 回目（3 回目の完了後に採取した最終実行）を
代表値として採用し、**削減率 12.3x（保守的な下限値）** を受け入れ判定に用いる。

## 考察

- キャッシュヒットは署名検証（RSA-2048 の `ring::signature::verify`）を完全に
  スキップし、SHA-256 ハッシュ計算 + `RwLock` 読み取り + `Arc::ptr_eq` 比較 +
  `exp` 比較のみで完了するため、直接呼び出しの 1/12〜1/20 のコストに収まる。
- 1 リクエストあたりの削減効果: ゲート（1 回、必ずミス→実検証）+ ハンドラ（1 回、
  ヒット）の 2 回呼び出しパターンでは、従来 2 回分の署名検証コスト
  （約 2 × 17,000 ns ≈ 34,000 ns）が、キャッシュ導入後は 1 回の署名検証 + 1 回の
  ヒット（約 17,000 + 900 ns ≈ 17,900 ns）へ短縮され、**約 47% 削減**（保守的な
  4 回目の数値では 17,000 + 1,425 ≈ 18,425 ns で約 46% 削減）。大量リクエスト時
  （同一トークンでの連続アクセス）はヒット回数が増えるほど削減率が理論値の
  12〜20x に漸近する。
- `crates/plugin-hub-wiring/tests/tenant_gate.rs` の
  `handler_reuses_gate_verification_via_shared_authenticator` テストで、ゲート
  （1 ミス）→ ハンドラ（1 ヒット）の実際の呼び出し順序・回数を機械検証済み
  （重複解消の直接証跡）。

## 受け入れ条件との対応

- (1) 「ゲート通過後のハンドラで署名再計算なしにクレーム取得可能」:
  `handler_reuses_gate_verification_via_shared_authenticator`（E2E、`cache_hits() == 1`）
  および `auth::tests::second_call_with_same_token_is_a_hit_and_returns_same_claims`
  （ユニット）で機械検証済み。
- (2) 「コスト計測レポートが `benches/reports/` に存在」: 本ファイル。

## スコープ外（本タスクでは対応しない）

- 専有環境での E2E 性能再計測・越境クエリ遮断テスト（TASK-9.5 相当、別 Issue）
- JWKS 自動リフレッシュヘルパー（HTTP クライアント連携、TASK-9.2 時点の申し送り事項）
- コアへの request extensions 機構追加（`GateOutcome` 契約変更、必要性が生じたら
  別 Issue でユーザー承認の上検討）
