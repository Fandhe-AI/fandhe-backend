# TLS 終端・multipart/form-data の v1 スコープ方針

イシュー [#322](https://github.com/Fandhe-AI/fandhe-backend/issues/322) 対応。
`docs/spec/04-requirements.md`（要件定義・submodule）の「除外事項（v1 スコープ外）」表に
本フレームワークでは項番が未定だった TLS 終端・multipart/form-data の 2 点の方針を明文化し、
upstream（[Fandhe-AI/fandhe-backend-spec](https://github.com/Fandhe-AI/fandhe-backend-spec)）の
除外事項表へ #8・#9 として追記する（[upstream PR #2](https://github.com/Fandhe-AI/fandhe-backend-spec/pull/2)）。

## 方針の要約

| 項目 | v1 方針 |
|------|--------|
| TLS 終端（HTTPS の直接終端） | フレームワーク本体では終端せず、リバースプロキシ / ロードバランサ（nginx・Caddy・クラウド LB 等）での終端を前提とする。フレームワークは背後の平文 HTTP のみを扱う |
| multipart/form-data のパース | v1 では解釈しない。body は既存の `MAX_BODY_BYTES`（1 MiB、`crates/http/src/body.rs`）内の raw バイト列として受理するのみ |

## 根拠

- **最小コア原則との整合**: rustls 等の TLS スタックをコアへ持ち込むと依存・攻撃表面・
  監査対象が増え、REQ-1（最小コア）・NFR-4（依存最小化）と衝突する
  （[[pay-for-what-you-use]]）。TLS 終端は責務としてリバースプロキシ層に委ねるのが
  素直な境界分割であり、フレームワーク自身が担う必然性がない
- **DoS 境界設計負担の回避**: multipart/form-data は境界（boundary）パース・part 数・
  part サイズ・ネスト深度の上限管理など、自前実装すると DoS 境界の設計・検証負担が
  大きく攻撃表面を増やす。既存の body 上限（1 MiB、chunked デコーダの DoS 上限 3 種、
  イシュー #181）は raw バイト列の受理のみを前提としており、multipart 解釈を追加すると
  この境界検証の前提が崩れる
- **既存実装事実との整合**: 2026-07-20 時点で `crates/http` に TLS・multipart の実装は
  存在しない。body は raw バイト列として `MAX_BODY_BYTES` 内で受理するのみ
  （`crates/http/src/body.rs:26`）。本方針は現状の実装をそのまま追認するものであり、
  既存の安全性方針（DoS 耐性・境界検証）を後退させない

## 除外事項表との対応関係

`docs/spec/04-requirements.md`「除外事項（v1 スコープ外）」表の既存項番 #6
（サービスメッシュ・mTLS・本番インフラ構成の選定）は**インフラ選定**の話であるのに対し、
新設した #8（TLS 終端）は**フレームワーク自身が終端機能を持つか否か**の話であり、
両者は別の論点として区別する（#8 の除外理由欄に交差参照を明記）。

- upstream 追記行: `docs/spec/04-requirements.md` 除外事項表 #8・#9
  （[upstream PR #2](https://github.com/Fandhe-AI/fandhe-backend-spec/pull/2)。
  マージ後は本体側 `docs/spec` submodule 参照を main の SHA へ更新する。詳細は
  イシュー #280 の先行事例（spec 側 PR → 本体側 PR で暫定ポイント → マージ後に
  再ポイント）と同一パターンを踏襲する）

## 個別要求が来た場合の判定指針

TLS 終端・multipart を求める個別の機能要求（例:「HTTPS を直接受けたい」「ファイル
アップロードを multipart で受けたい」）を受けた場合、
[[feasibility-guardrail]] の 3 軸判定に本方針を接続する。

- 両者とも「v1 除外」が確定方針であるため、単純な要求（feature 追加の要望のみ）は
  「安全性方針との衝突」カテゴリではなく、**本方針の未整備領域への機能追加要求**として
  扱う。3 軸のうち「影響範囲が許容内か」を本方針（コアへの重依存持ち込み回避）に照らして
  再判定し、コアへの直接実装を求める要求は「不可・要エスカレーション」（未定義依存型:
  実装方式〔プラグイン化 vs コア組み込み〕が要求文面から未確定）とする
- `tls` feature プラグイン・`multipart` feature プラグインとしての実装要求（本方針の
  「将来の可能性」に沿う形）であれば、pay-for-what-you-use 準拠（feature 無効時は
  依存ゼロ）を着手条件とした「条件付き可」に倒せる余地がある。ただし多重防御崩れ
  （multipart の DoS 上限未設計等）が要求に含まれる場合は「安全性方針との衝突」
  カテゴリで不可側に倒す

## 参照

- 要件定義: [`docs/spec/04-requirements.md`](../spec/04-requirements.md) 除外事項表
  （submodule。upstream マージ後に本表の #8・#9 が反映される）
- upstream PR: [Fandhe-AI/fandhe-backend-spec#2](https://github.com/Fandhe-AI/fandhe-backend-spec/pull/2)
- 対応可否判定ガードレール: [`docs/design/feasibility-guardrail.md`](./feasibility-guardrail.md)
- pay-for-what-you-use 原則: [`.claude/rules/pay-for-what-you-use.md`](../../.claude/rules/pay-for-what-you-use.md)
- body 上限の実装事実: `crates/http/src/body.rs`（`MAX_BODY_BYTES`）
- chunked デコーダの DoS 上限: イシュー #181
