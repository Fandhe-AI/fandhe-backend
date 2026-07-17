# 拡張点への変更影響範囲閉包の実例検証（TASK-13.1 / #49）

REQ-13「変更影響範囲の機械判定構造」（`docs/spec/04-requirements.md`）の受け入れ基準
「新規プロトコル追加が既存拡張点に閉じるか、閉じない場合はその理由が設計文書に明記される」を、
WebSocket・WebRTC・GraphQL の 3 実例（実際に merge されたコミット）で検証した結果を記録する。

**結論（要約）**: WebSocket・GraphQL の 2 実例は拡張点へ完全に閉包することを確認した。
WebRTC は 1 ファイルが閉包の外に出たが、その理由を本書に明記しており、これは REQ-13
受け入れ基準の「閉じる」側条件ではなく「閉じない場合は理由を明記する」側条件を満たす
形で REQ-13 を充足している（6 節参照）。3 実例中 1 件が機械判定で FAIL になったこと
自体は、判定スクリプトが実際に非自明な差異を検出できている証跡でもある
（3 実例すべてが無条件 PASS するようカテゴリを恣意的に広げていないことの裏付け）。

## 1. 目的・判定基準

3 種の拡張点（`Middleware` / `UpgradeHandler` / `RequestGate`）は、実装上は
`crates/core/src/plugin.rs` の固定シーム（`try_intercept` / `try_handle_upgrade`）に
集約されている（`docs/design/plugin-boundary.md` 3〜5 節）。本書では「閉包」を次のように
定義する。

> 新規プロトコル追加コミットの変更ファイルが、以下 A〜D の 4 カテゴリのいずれかに
> 全て収まり、かつコアループ（`crates/core/src/server.rs` の `handle_connection`）の
> cfg-free 原則・`crates/http` / `crates/routes` の無変更が保たれていること。

| カテゴリ | 内容 |
|---|---|
| A. プラグインクレート内 | `crates/plugin-*/**`（テストを除く） |
| B. コア側許容シーム | `crates/core/Cargo.toml`・`crates/core/src/plugin.rs`・`crates/core/src/server.rs`・`crates/core/src/lib.rs` |
| C. テスト | `crates/core/tests/**`・`crates/plugin-*/tests/**` |
| D. ドキュメント・運用 | `docs/**`・`scripts/**`・`CLAUDE.md`・`AGENTS.md`・`.github/**`・`deny.toml`（依存ライセンス許可リスト。理由は 4.2 節） |
| E（違反） | 上記いずれにも該当しない残り（`crates/http/**`・`crates/routes/**`・`crates/core/src/` のその他ファイル 等） |

機械判定は `scripts/extension-closure-check.sh --commit <sha>` で行う（2 節）。
E に該当するファイルが 1 件でもあれば非 0 終了・当該ファイルを列挙する。判定不能
（sha 不正・git 失敗・空 diff）はフェイルクローズで FAIL とする。

## 2. 検証対象コミット・再現コマンド

前提タスク（すべて CLOSED、origin/main 上の merge commit）:

| プロトコル | タスク / Issue | PR | merge commit |
|---|---|---|---|
| WebSocket | TASK-4.1 / #22 | #137 | `3ae6d11` |
| WebRTC | TASK-8.1 / #26 | #138 | `1877cfa` |
| GraphQL | TASK-5.1 / #38 | #144 | `6a6fb9c` |

再現コマンド:

```bash
bash scripts/extension-closure-check.sh --commit 3ae6d11   # WebSocket
bash scripts/extension-closure-check.sh --commit 1877cfa   # WebRTC
bash scripts/extension-closure-check.sh --commit 6a6fb9c   # GraphQL
```

## 3. 検証結果表

### 3.1 WebSocket（`3ae6d11`）— 対応拡張点: `UpgradeHandler`（Upgrade 型シーム）

| 分類 | ファイル |
|---|---|
| A | `crates/plugin-websocket/Cargo.toml`・`src/config.rs`・`src/error.rs`・`src/handshake.rs`・`src/lib.rs`・`src/session.rs` |
| B | `crates/core/Cargo.toml`・`src/lib.rs`・`src/plugin.rs`・`src/server.rs` |
| C | `crates/core/tests/websocket_upgrade.rs`・`websocket_upgrade_disabled.rs`・`crates/plugin-websocket/tests/handshake_e2e.rs` |
| D | `CLAUDE.md`・`docs/dep-impact/records.md`・`docs/design/plugin-boundary.md`・`scripts/dep-direction-check.sh` |
| E | なし |

**判定: PASS（閉包）。** `crates/core/src/server.rs` の変更は「シームのシグネチャ変更」
（`plugin-boundary.md` 5.1 節）を含むが、これは Upgrade 型パターン確立という**初例ゆえの
意図的逸脱**であり、5.1 節に理由（残余バイト列 `leftover` の引き継ぎ・`&Server` 経由の
cfg-gated 設定アクセスの一般化）が明記済みである。B カテゴリ（コア側許容シーム）の
範囲内の変更として扱う。

### 3.2 WebRTC（`1877cfa`）— 対応拡張点: パスインターセプト型シーム（`try_intercept`）

| 分類 | ファイル |
|---|---|
| A | `crates/plugin-webrtc/Cargo.toml`・`src/config.rs`・`src/handler.rs`・`src/lib.rs` |
| B | `crates/core/Cargo.toml`・`src/lib.rs`・`src/plugin.rs`・`src/server.rs` |
| C | `crates/core/tests/plugin_boundary_webrtc.rs`・`crates/plugin-webrtc/tests/webrtc_datachannel.rs` |
| D | `CLAUDE.md`・`deny.toml`・`docs/dep-impact/records.md`・`docs/design/plugin-boundary.md`・`scripts/dep-direction-check.sh` |
| **E** | **`crates/http/src/response.rs`** |

**判定: FAIL（部分閉包違反、理由明記あり）。** `crates/http/src/response.rs` の
`reason_phrase` 固定テーブルへ、`plugin-webrtc` が同時接続数上限到達時に払い出す
`503` の reason phrase を追加する 1 行の変更が含まれる。

- **なぜ閉じなかったか**: `reason_phrase` はコアループ・`bf_routes::Router::dispatch`・
  `plugin-webrtc-proxy` の複数箇所が共有するステータスコード→文言の静的対応表であり、
  `crates/http` に一元管理されている（同ファイル該当 doc comment 参照）。新しい HTTP
  ステータスコードを払い出すプラグインを追加すると、この共有テーブルへのエントリ追加が
  必要になる。これはプラグインの実装ロジック（`try_intercept` の中身）を `crates/http`
  や `crates/routes` へ漏出させるものではなく、**「新しいステータスコードの英語文言を
  1 件追加する」という定数データの追加**に限定される。
- **正当性の根拠**: (1) `crates/http` から `crates/plugin-webrtc` への依存は発生しない
  （依存方向は逆のまま、`scripts/dep-direction-check.sh` で検証済み）。(2) 変更は
  1 テーブルへの 1 エントリ追加のみで、コアループの cfg-free 原則（`handle_connection`
  無変更）は保たれている。(3) 同種の変更は WebSocket・GraphQL では発生していない
  （両者とも新規ステータスコードを払い出さないため）。
- **REQ-13 の目的（変更影響範囲の機械判定可能性）への影響**: 影響は限定的だが、
  機械判定スクリプトが「E」として検出する以上、**現行の A〜D ホワイトリストのままでは
  WebRTC 実例は閉包しない**という事実は隠さず記録する。これは REQ-13 が求める
  「閉じない場合はその理由を設計文書に明記」を満たす形で本節に記載した。
  是正（`reason_phrase` テーブルの拡張性向上、または B カテゴリへの `crates/http/src/response.rs`
  の限定的な追加是非の検討）は 7 節のとおりスコープ外として切り出す。

### 3.3 GraphQL（`6a6fb9c`）— 対応拡張点: パスインターセプト型シーム（`try_intercept`）

| 分類 | ファイル |
|---|---|
| A | `crates/plugin-graphql/Cargo.toml`・`src/lib.rs` |
| B | `crates/core/Cargo.toml`・`src/plugin.rs`・`src/server.rs` |
| C | `crates/core/tests/plugin_graphql_boundary.rs` |
| D | `CLAUDE.md`・`deny.toml`・`docs/dep-impact/records.md`・`docs/design/plugin-boundary.md` |
| E | なし |

**判定: PASS（閉包）。**

### 3.4 パスインターセプト型が「3 trait のいずれでもない」点について

GraphQL・WebRTC はいずれも `UpgradeHandler` / `Middleware` / `RequestGate` の**いずれの
trait 実装でもない**。これは `plugin.rs::try_intercept` という TASK-2.1（#18）で確立した
固定シームへの cfg-gated 分岐として実装されており、3 trait そのものではなく「3 trait と
並ぶ第 4 のシーム」に閉じている（`docs/design/plugin-boundary.md` 4 節）。

REQ-13 の目的は「拡張点（シーム）への集約により変更影響範囲を機械判定可能にすること」
であり、`try_intercept` は `try_handle_upgrade` と同様に**シグネチャが固定された単一の
分岐点**である。GraphQL・WebRTC の 2 実例が共にこの単一シームへ cfg-gated 分岐を
追加するだけで閉じている（GraphQL は E 0 件で完全 PASS、WebRTC は前述の 1 件のみ）
ことから、パスインターセプト型シームは REQ-13 が要求する「新規プロトコル追加の影響範囲を
機械判定できる」という目的を、3 trait とは別の形で満たしていると評価する。

## 4. 考察

### 4.1 閉包の実証度合い

3 実例中 2 件（WebSocket・GraphQL）は A〜D に完全に収まり閉包が確認できた。WebRTC は
1 ファイル（`crates/http/src/response.rs`）が E に該当したが、上記 3.2 節のとおり
プラグイン実装ロジックの漏出ではなく、共有定数テーブルへの 1 エントリ追加という限定的な
逸脱であり、理由を本書に明記した。

### 4.2 D カテゴリへの `deny.toml` 追加について

`deny.toml`（依存ライセンス許可リスト）は WebRTC・GraphQL の両方で変更されているが、
リポジトリ計画時点のホワイトリスト定義には明記されていなかったため、本検証を通じて
D カテゴリへ追加した。`deny.toml` は特定クレートの実装ではなく workspace 全体の
依存ガバナンス設定であり、新規プラグインが新しい推移依存のライセンスを持ち込んだ際の
`scripts/dep-audit.sh` 運用上の副作用（`docs/design/plugin-boundary.md` 345 行目参照）
であって、拡張点の設計失陥ではない。この判断はホワイトリストの精緻化として本書に記録し、
`scripts/extension-closure-check.sh` のコメントにも反映済み。

**`response.rs`（E 判定）との対比**: `deny.toml` と `crates/http/src/response.rs` は
どちらも「新規プラグイン追加に伴う副作用」という点では類似するが、閉包判定上の扱いは
異なる。`deny.toml` はリポジトリ直下の workspace 全体向け依存ガバナンス設定であり、
特定クレートの実装コードではない（`crates/http`・`crates/routes`・`crates/core` の
いずれにも属さない）。対して `response.rs` は「`crates/http` に変更があってはならない」
という本タスクの核心的な検証対象クレートに属する実装ファイルそのものである。前者は
「どこにも属さない運用ファイル」、後者は「閉じているべきクレートに属する実装ファイル」
という構造的な違いがあるため、前者を D（許容）・後者を E（違反候補）として区別して
扱う。

## 5. 再現結果（実行ログ要約）

```
$ bash scripts/extension-closure-check.sh --commit 3ae6d11
...
[PASS] 閉包 — 全 17 件が A〜D に収まっています
[RESULT] PASS

$ bash scripts/extension-closure-check.sh --commit 1877cfa
...
[FAIL] 閉包 — A〜D のいずれにも該当しないファイルが 1 件あります（拡張点への閉包違反）
  [E] crates/http/src/response.rs
[RESULT] FAIL

$ bash scripts/extension-closure-check.sh --commit 6a6fb9c
...
[PASS] 閉包 — 全 10 件が A〜D に収まっています
[RESULT] PASS
```

## 6. 結論

- WebSocket（`UpgradeHandler`）・GraphQL（パスインターセプト型）の 2 実例は拡張点へ
  完全に閉包することを実例で確認した。
- WebRTC（パスインターセプト型）は `crates/http/src/response.rs` への 1 行変更のみが
  A〜D の外に出たが、これはプラグイン実装ロジックの漏出ではなく共有定数テーブルへの
  データ追加であり、理由をこの設計文書に明記した（REQ-13 受け入れ基準を満たす形）。
- パスインターセプト型シーム（`try_intercept`）は 3 trait そのものではないが、
  `try_handle_upgrade` と同様の「シグネチャ固定の単一シーム」として機能しており、
  REQ-13 が求める変更影響範囲の機械判定可能性という目的を実質的に満たしている。

## 7. TASK-13.2（#50）への引き継ぎ事項

- doc コメントでの依存グラフ機械可読化（拡張点・シームと実装クレートの対応を
  doc comment から機械的に辿れるようにする運用）は本タスクのスコープ外
- 本書 Step 4・3.2 節の判定を CI 上の受け入れテストとして常設運用する仕組み
  （`extension-closure-check.sh` を新規プラグイン追加 PR に対して自動実行する等）は
  TASK-13.2 のスコープ
- `crates/http/src/response.rs` の `reason_phrase` テーブルが将来さらに複数プラグインの
  ステータスコードを蓄積し続ける場合の設計見直し（例: プラグイン側からの reason phrase
  提供に切り替える等）は、真の閉包違反の是正として別 Issue 化を検討する候補
  （8 節参照、本 PR では起票せず提案に留める）

## 8. スコープ外（別 Issue 化候補、本 PR では起票しない）

- 3.2 節で述べた `crates/http/src/response.rs` の閉包逸脱の是正案（reason phrase
  提供方式の見直し）。影響は軽微（1 テーブルへの 1 エントリ追加が今後も想定されうる
  程度）だが、`.claude/rules/out-of-scope-tracking.md` に従いユーザー承認を得てから
  Issue 化を検討する
