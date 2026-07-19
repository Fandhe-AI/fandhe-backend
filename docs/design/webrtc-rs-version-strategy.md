# webrtc-rs バージョン戦略

- **対応タスク**: [TASK-8.3](../spec/05-tasks.md)「webrtc-rs バージョン戦略の策定」（#28、前提タスク TASK-8.1）
- **対応要件**: [REQ-8](../spec/04-requirements.md)「WebRTC プラグイン（別プロセス切り出し設計）」
- **前提タスク**: TASK-8.1（PoC-5、`docs/spec/03-poc/webrtc-plugin/`）、TASK-8.2（[別プロセス切り出し設計](./webrtc-process-isolation.md)、設計確定）
- **ステータス**: ドラフト（担当は `docs/spec/05-tasks.md` 上「人間」。自動運転モードでの実装であるため、
  本ドキュメントは安全側の保守的推奨として作成したドラフトであり、**最終承認は人間レビュー（本タスクの PR レビュー）で行う**）

本フレームワークの正式名称は `fandhe-backend`（#200 で確定）のため、本ドキュメントでは「本フレームワーク（fandhe-backend）」と表記する。

## 1. 背景

`webrtc-rs`（`webrtc` クレート）は本フレームワークの WebRTC プラグイン PoC（PoC-5）で採用した実装であり、
過渡期にある（[PoC README「1. `webrtc-rs`（`webrtc` クレート）の成熟度・保守体制の調査」](../spec/03-poc/webrtc-plugin/README.md)15〜16 行目、
[「発見事項」](../spec/03-poc/webrtc-plugin/README.md)113 行目）。

- 2026-01-31、`webrtc` v0.17.0 がリリースされ、Tokio に密結合した現行アーキテクチャの**最終機能リリース**
  （feature freeze）と位置づけられた。以降 v0.17.x ブランチはバグ修正のみを受け付ける**保守モード**であり、
  Tokio ベースの本番利用ではこの系列が推奨されている
- `master` ブランチでは Sans-I/O アーキテクチャへの移行が進行中で、2026-03 に `webrtc` v0.20.0-alpha.1 が
  プレリリースされた（ランタイム非依存、Tokio/smol 両対応）

### 実装時点（2026-07-17）の再確認結果

`reference-researcher` 相当の Web 調査（WebSearch）で以下を確認した（確認日: 2026-07-17）。

- crates.io 上の `webrtc` クレート最新版は引き続き **v0.17.1**（[crates.io/crates/webrtc](https://crates.io/crates/webrtc)）。
  PoC-5 実施時点（2026-07-08）から変化なし
- **重要な更新**: Sans-I/O 移行は `webrtc` クレート自体の v0.20 系としてではなく、**別クレート `rtc`** として
  切り出されて進められている。2026-01-04 付けの WebRTC.rs 公式アナウンス
  （[Announcing rtc v0.3.0: Sans-I/O WebRTC Stack for Rust](https://webrtc.rs/blog/2026/01/04/announcing-rtc-v0.3.0.html)）
  によれば、`webrtc`（async/await・Tokio 結合）と `rtc`（Sans-I/O・ランタイム非依存）は「競合ではなく相補的」な
  2 系列として並行維持される方針であり、同アナウンスには `webrtc` クレートの v0.20 化について明示的な言及がない。
  すなわち、PoC-5 実施時点で観測された「v0.20.0-alpha.1」は `webrtc` 本体のメジャーバージョンではなく、
  実質的に `rtc` プロジェクトへ発展的に分離した可能性が高い（要出典追認。`webrtc-rs/webrtc` リポジトリの
  `master` ブランチ・タグ履歴の直接確認までは実施していない）
- webrtc 系クレート（`webrtc`・`webrtc-ice`・`webrtc-sctp`・`webrtc-mdns` 等）を対象にした RUSTSEC advisory は
  今回の調査範囲では確認できなかった（[RustSec Advisory Database](https://rustsec.org/advisories/) の
  webrtc 系エントリ有無は本ドキュメント作成時点の簡易検索によるものであり、`cargo audit` の実機実行による
  確定確認ではない）

この再確認結果は「2. 決定事項」の当面採用判断（v0.17.x 継続）を追認する方向であり、v0.20 系（または `rtc`
クレート）が安定版としてまだ存在しないという意味で、当初の移行トリガー設計に変更を要しない。ただし
「`webrtc` v0.20」ではなく「`rtc` クレートへの分離」という構造変化は移行トリガーの評価対象を精緻化する必要が
あるため、「2. 決定事項」の移行トリガー基準に反映する。

## 2. 判断の前提（影響範囲の整理）

[TASK-8.2 別プロセス切り出し設計](./webrtc-process-isolation.md)により、本フレームワーク側の実装
（`crates/plugin-webrtc-proxy`）は `webrtc-rs` に**一切依存しない**（同ドキュメント「6. pay-for-what-you-use
検証方針」、`cargo tree -p fandhe-backend-plugin-webrtc-proxy` で webrtc 系依存 0 件を検証可能）。したがって、
本バージョン戦略が実際に影響する範囲は次の 2 つに限られ、**フレームワーク本体の対応 crate 一覧・API には
一切影響しない**。

1. **独立 WebRTC サービス**（Fandhe の共用 WebRTC マイクロサービス構想側、または実装フェーズで暫定的に
   同梱する参照実装）: `webrtc-rs` を直接抱える側
2. **PoC 参照実装**（`docs/spec/03-poc/webrtc-plugin/`、v0.17.1 採用）: 事後書き換えしない記録であり、
   本戦略のスコープ外（実装時点の判断が異なっても PoC 記録自体は不変）

シグナリング HTTP 契約（`POST /rtc/offer`、[webrtc-process-isolation.md「4. 連携インターフェース」](./webrtc-process-isolation.md)）
が本フレームワーク利用者から見た安定境界であるため、独立 WebRTC サービス側の `webrtc-rs` メジャーバージョン
移行は、本フレームワーク利用者への破壊的変更には**ならない**。これが本戦略を「担当: 人間」ながらも実装を
ブロックせずドラフトとして先行策定できる根拠である。

## 3. 決定事項（推奨案・安全側）

- **当面 v0.17.x（保守モード）を継続採用**する。独立 WebRTC サービスの初期実装（実装フェーズで着手する場合）も
  v0.17.x 系（`~0.17` 相当、パッチ更新のみ追随）で行う
- **Sans-I/O 系（`webrtc` v0.20 系、または分離先の `rtc` クレート）は安定版（非 alpha/beta）リリースまで不採用**
  とする。alpha/プレリリース段階での採用は、API 不安定・監査実績不足・サプライチェーンリスク（未成熟な
  crates.io 配布物への依存）の観点で見送る
- **移行トリガー基準**を次のとおり明文化する。いずれか成立時に再評価する:
  1. Sans-I/O 系（`webrtc` v0.20 系、または `rtc` クレート）の**安定版**（非 alpha/beta）がリリースされた場合
  2. v0.17.x 系でセキュリティ修正が提供されなくなった、または未修正の RUSTSEC advisory が webrtc 系クレートに
     発生した場合（`cargo audit` の CI schedule 実行で検知）
  3. スコープ外機能（トリクル ICE・STUN/TURN・複数データチャネル・接続クローズ時のリソース解放、「4.」参照）の
     実装において v0.17.x 系 API では実現不可能、または著しく非効率であることが判明し、Sans-I/O 系 API が
     必須になる場合
- **破壊的変更の許容度**: 移行は独立 WebRTC サービス内部に閉じるため、独立サービス側の実装詳細としては
  破壊的変更を許容する。ただし本フレームワーク側のシグナリング HTTP 契約（`POST /rtc/offer`）を不変条件とする
  （「2. 判断の前提」参照）
- **再評価サイクル**: 四半期ごとの定期再評価に加え、`cargo audit`（CI schedule 実行）で webrtc 系 advisory を
  検知した場合は即時再評価する

## 4. スコープ外機能の実装フェーズでの扱い

トリクル ICE・STUN/TURN 対応（NAT 越え）・複数データチャネル・接続クローズ時のリソース解放処理は、REQ-8 の
スコープ外事項（[04-requirements.md](../spec/04-requirements.md)205 行目、[webrtc-process-isolation.md「2. 設計目標・非目標」](./webrtc-process-isolation.md)）
として独立 WebRTC サービスの実装フェーズで扱う。

- v0.17.x 系 API で実装可能なものはそのまま v0.17.x 系で実装する。PoC-5 の発見事項
  （[PoC README「発見事項」](../spec/03-poc/webrtc-plugin/README.md)114〜117 行目）によれば、トリクル ICE
  （`on_ice_candidate` ごとの逐次シグナリング）・複数データチャネル・接続クローズ時の解放処理
  （`on_peer_connection_state_change` の `Closed`/`Failed` ハンドリング）はいずれも v0.17.x 系 API の範囲で
  実現可能と見込まれる
- Sans-I/O 系 API を前提とした再設計は行わない。「3. 決定事項」の移行トリガーが成立した後、必要であれば
  Sans-I/O 系での再評価を別途行う

## 5. セキュリティ考慮（OWASP Top 10 観点）

- **A06 脆弱・古いコンポーネント**: v0.17.x は保守モード（バグ修正のみ）のため、セキュリティ修正の提供が
  細る・止まるリスクがある。`cargo audit`（CI schedule 実行）による RUSTSEC 監視を常設し、「未修正 advisory
  発生」を移行トリガーに含める（「3. 決定事項」トリガー基準 2）。一方で Sans-I/O 系（alpha 相当）は未成熟な
  API・監査実績不足のリスクがあるため安定版まで不採用とし、両側のリスクを比較したうえでの判断として記録する
- **A08 ソフトウェア・データ整合性（サプライチェーン）**: 依存採否は `cargo deny check`（ライセンス・出所）
  通過を条件とし、alpha/プレリリース版の本番採用を禁止する方針とする
- **攻撃表面最小化（本フレームワークの核）**: 別プロセス切り出し（TASK-8.2）により `webrtc-rs` の巨大依存
  （+189 クレート・`unsafe` Functions 約 2.2 倍、[webrtc-process-isolation.md「1. 背景」](./webrtc-process-isolation.md)）
  が本フレームワーク側の監査対象に入らない構造を維持することを、本バージョン戦略の**不変条件**とする。
  `cargo tree -p fandhe-backend-plugin-webrtc-proxy` で webrtc 系依存 0 件であることの継続検証は TASK-8.4（#29）で実施する
- **シークレット混入防止**: 本ドキュメントにトークン・内部 URL 等の機密情報は含まない
- **DoS・入力検証**: シグナリング境界（`POST /rtc/offer`）の SSRF 対策・ボディサイズ上限・タイムアウトは
  [webrtc-process-isolation.md「5. セキュリティ設計」](./webrtc-process-isolation.md)・
  `crates/plugin-webrtc-proxy` 側の責務であり、本ドキュメントでは参照に留めてスコープを混入させない

## 6. 関連タスク・スコープ外

- **TASK-8.4**（[#29](https://github.com/Fandhe-AI/fandhe-backend/issues/29)）: 本バージョン戦略を前提に
  WebRTC プラグインの攻撃表面を再評価し、受け入れテストを実施する
- **[#96](https://github.com/Fandhe-AI/fandhe-backend/issues/96)**（フレームワーク本体の semver・破壊的変更
  ポリシー）: 本ドキュメントは独立 WebRTC サービス側が依存する `webrtc-rs` のバージョン戦略であり、
  フレームワーク本体の semver とは**別軸**。参照のみに留め、判断内容を混在させない
- **独立 WebRTC サービスの実装・Sans-I/O 系への実移行作業**: 「3. 決定事項」の移行トリガー成立時に、
  Fandhe の共用 WebRTC マイクロサービス構想側の実装として別途着手する（本タスクのスコープ外）
