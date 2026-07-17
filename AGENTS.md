# AGENTS.md

## 文書の位置づけ

本リポジトリで作業するすべての AI エージェント・開発者が従う設計規約集。
全体の運用ガイドは `CLAUDE.md`、Rust コーディング規約の詳細は `.claude/rules/`
（特に [coding-rust.md](.claude/rules/coding-rust.md)）を参照し、本書は
`CLAUDE.md` / `.claude/rules/` と内容を重複させず、実装コード（`crates/**`）から
直接参照される横断的な設計規約のみを記載する。

## 規約: ミドルウェア非同期 I/O 必須化

TASK-2.3（`docs/spec/05-tasks.md`、Phase 1 / MS-1、親 Issue #4、前提 TASK-2.1 #18）
対応。`docs/spec/04-requirements.md` REQ-2 受け入れ基準・NFR-7 を満たす規約文書。

### 規約本文

全リクエストに介入する `Middleware` 実装（`crates/core/src/extension.rs` の
`Middleware` trait、`on_request` / `on_response`）は**非同期・バッファ済み I/O を
既定**とする。同期ブロッキング I/O 実装（同期 `eprintln!`・同期ファイル書き込み・
`std::net` 直接利用等）は**不採用**とする。

`Middleware` trait 自体は dyn 互換性（`Box<dyn Middleware>` としてコアループが
拡張点を保持する構成）を保つため `async fn` を持たない同期 API として定義される
（`crates/core/src/extension.rs` モジュール doc「非同期・I/O に関する規約」節）。
本規約はこの同期 API の**制約下で守るべき実装契約**であり、trait のシグネチャ変更
を求めるものではない。

### 実装パターン

I/O が必要な実装は、フック（`on_request` / `on_response`）内では非同期チャネルへの
送信、またはアトミックカウンタの更新等の**非ブロッキング操作に留め**、実際の I/O
（ファイル書き込み・ネットワーク送信等）は別タスク（バックグラウンドタスク・
`tracing-appender` の non-blocking writer 等）に委譲する。

### 根拠（PoC-3 実測、`docs/spec/03-poc/plugin-mechanism/README.md`）

全リクエストに介入するミドルウェア型プラグイン（ロギング）を素朴な同期 I/O
（リクエストごとの同期 `eprintln!`）で実装すると、`/health` の RPS が
**725,024 → 44,108 RPS（無効時比 25.0%）** まで劣化した。同一の `Middleware`
trait 実装のまま I/O を停止し、アトミックカウンタの更新のみに切り替えて計測
（`ACCESS_LOG_QUIET=1`）すると **177,549 RPS（無効時比 100.5%）** まで回復した。

この切り分けにより、劣化要因は「`Middleware` trait 呼び出し（動的束縛）のコスト
自体」ではなく「プラグインが選んだ I/O 実装の質（同期か非同期か）」であることが
実証された。

補足として、PoC-10（`docs/spec/04-requirements.md` REQ-10）でも同旨の実測がある。
可観測性ミドルウェアを同期 writer で実装した場合に RPS が **63.0% 劣化**すること
に加え、非同期 writer に切り替えても span/event 生成の CPU コストにより RPS が
31.6% 劣化する事例が確認されており、**非同期 I/O 化だけでは pay-for-what-you-use
の性能目標を満たさない場合がある**（サンプリング・イベント数削減・高頻度パス除外
等の追加対策は REQ-10 側のスコープであり、本規約は「同期 I/O の不採用」という
最小限の必須要件を定めるものである）。

### 出典リンク

- `docs/spec/03-poc/plugin-mechanism/README.md`（PoC-3 性能比較表・発見事項）
- `docs/spec/02-poc-plan.md`（PoC-3 計画）
- `docs/spec/04-requirements.md`（REQ-2・NFR-7、参考: REQ-10・PoC-10）
- `docs/spec/05-tasks.md`（TASK-2.3）
- `crates/core/src/extension.rs`（`Middleware` trait 定義・同旨の契約を doc comment に記載）

### 適用範囲と検証責務

標準提供ミドルウェア有効化時のコア RPS 劣化は 5% 以内を維持する（NFR-7 受け入れ
基準）。レビュー時の本規約準拠確認は `reviewer` / `plugin-builder`、性能検証は
`bench-builder` が担う（[delegation-impl.md](.claude/rules/delegation-impl.md)）。

### 可用性・可観測性に関する注記

- **リソース枯渇（DoS）耐性**: 全リクエストのホットパスに載るミドルウェアが同期
  I/O を行うと、スロー I/O（ディスク詰まり・パイプブロック等）発生時にワーカー
  スレッドが枯渇し、サービス全体が応答不能に陥りうる。本規約はこのリスクを構造的
  に排除する（[security.md](.claude/rules/security.md) の「リソース枯渇（DoS）」
  観点）。
- **ログ欠落の許容可否**: 非同期・バッファ済みログはバックプレッシャ時にイベント
  欠落（drop）が起こりうる。セキュリティ監査イベント等、欠落を許容できないログの
  扱いは、標準ロギング／トレーシング実装（REQ-10・`plugin-tracing` 系タスク）側
  の設計事項として別途定める。本規約はこの論点を暗黙に決定しない。

## 規約: WebRTC の攻撃表面と「使う/使わない」サービスの安全性方針

TASK-8.4（`docs/spec/05-tasks.md`、Phase 2 / MS-2、#29）対応。`docs/spec/04-requirements.md`
REQ-8（WebRTC）受け入れ基準・NFR-6（拡張の非侵襲性）を満たす運用規約文書。

### 背景: 2 クレートの対照

backend-framework は WebRTC を 2 つの独立クレートで提供し、**クレート境界で完全に
分離**する（相互 path 依存なし。`docs/dep-impact/records.md` の TASK-8.4 エントリで
機械検証済み）。

| クレート | feature | 依存モデル | 攻撃表面 |
|---------|---------|-----------|---------|
| `crates/plugin-webrtc` | `webrtc` | `webrtc-rs`（0.17.1 系）を**プロセス内**に直接組み込む（in-process） | 大（`webrtc` feature 単体で `cargo tree -p backend-framework-core --features webrtc` に webrtc 系依存 23 件、release バイナリサイズ約 11 倍、TASK-8.4 実測。`docs/dep-impact/records.md`） |
| `crates/plugin-webrtc-proxy` | `webrtc-proxy` | `webrtc-rs` に**一切依存しない**軽量シグナリングプロキシ。重い WebRTC サービスは別プロセスへ切り出す | 小（`webrtc-rs` 依存が本体プロセスに一切現れない） |

`crates/core/src/plugin.rs` の `try_intercept` は両 feature が同時に有効な場合
（`--all-features` CI 構成）、`webrtc-proxy` を先に評価する（REQ-8 の MVP 推奨方式を
優先する運用判断。両方を `Server` に登録した場合は `webrtc-proxy` が優先され、
`webrtc` 側の設定は評価されない）。

### 安全性方針

- **WebRTC を使わないサービス**: `webrtc`・`webrtc-proxy` のどちらの feature も有効化
  しない。依存・`unsafe`・バイナリ増をゼロに保つ（pay-for-what-you-use、
  [pay-for-what-you-use.md](.claude/rules/pay-for-what-you-use.md)）。`cargo tree -p
  backend-framework-core` にいずれの feature 無効時も webrtc 系依存が一切現れないこと
  を維持する。
- **WebRTC を使うサービス**: 可能な限り `plugin-webrtc-proxy`（`webrtc-proxy` feature）
  による**別プロセス切り出し**を第一選択とする。`webrtc-rs` の巨大な依存グラフ・
  パーサ群をコアプロセスから隔離し、脆弱性発生時の影響範囲・監査対象を限定できる。
- **in-process `plugin-webrtc`（`webrtc` feature）を選ぶ場合**: 別プロセス切り出しの
  運用コスト（プロセス間通信・デプロイ構成の複雑化）が許容できない場合に限り検討する。
  有効化すると `webrtc-rs` の巨大な依存グラフ・パーサ群がコアプロセスに直接組み込まれ、
  ICE 接続性チェックはクライアント SDP 由来のアドレスへ UDP 送信を発生させ得る（WebRTC
  の構造上不可避）。STUN/TURN は既定で設定しない（`RTCConfiguration::default()`）。
  Offer サイズ上限・接続数上限（503 フェイルクローズ）・シグナリングタイムアウト
  （504）は維持されている（`crates/plugin-webrtc/tests/attack_surface.rs` で受け入れ
  観点から再アサート済み）が、依存グラフそのものの大きさは変わらない。

### NFR-6（無関係パスへの性能影響）に関する留意事項

NFR-6 は「パス一致時のみ介入する拡張点は、無関係なパスへの RPS・レイテンシ影響が
誤差範囲内（100.3〜100.8%相当）である」ことを求める。この帯は GraphQL（PoC-3、依存
インパクトが軽微なパスインターセプト型）由来の実測に基づく。TASK-8.4 の empirical
計測（`benches/webrtc-nfr6-bench.sh`、`benches/reports/task-8.4-webrtc-nfr6.md`）では、
`webrtc` feature 有効時の無関係パス（`GET /`）RPS が baseline 比おおむね 94〜95%、
p95 レイテンシがおおむね 106〜108% となり、狭義の 100.3〜100.8% 帯には収まらなかった。
`try_intercept` 自体は対象外パスに対して 1 回のパス比較のみでフォールスルーするため
（`crates/core/src/plugin.rs`）、この差は拡張点の呼び出しコストではなく、バイナリ
サイズが約 11 倍に達すること（icache/TLB 圧迫等）に起因すると考えられる。**WebRTC を
使うサービスがこの性能影響を避けたい場合も、`plugin-webrtc-proxy` による別プロセス
切り出しが有効な緩和策となる**（プロキシプロセスとコアプロセスが分離するため、コア
プロセスのバイナリサイズ・性能特性は影響を受けない）。

### 出典リンク

- `docs/design/webrtc-process-isolation.md`（別プロセス切り出しの設計判断）
- `docs/design/webrtc-rs-version-strategy.md`（`webrtc-rs` バージョン戦略、TASK-8.3）
- `docs/acceptance/req8-webrtc-attack-surface.md`（TASK-8.4 攻撃表面評価・受け入れ判定）
- `docs/dep-impact/records.md`（依存インパクト計測記録）
- `docs/spec/04-requirements.md`（REQ-8・NFR-6）
- `docs/spec/05-tasks.md`（TASK-8.1〜TASK-8.4）

### 適用範囲と検証責務

`webrtc`/`webrtc-proxy` 両 feature の依存完全除外・クレート境界分離の機械検証は
`scripts/accept/webrtc-accept.sh`、NFR-6 の empirical 計測は `bench-builder` が担う
（[delegation-impl.md](.claude/rules/delegation-impl.md)）。
