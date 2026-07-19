# 第三者検証のモデル多様性制約 — 恒久追跡

TASK-12.4（#45、REQ-12、Conditional Go 条件 (3)）に付随する制約「別ベンダー LLM・人間被験
による第三者検証は未実施」を、実施可能になるまで追跡し続けるためのイシュー #262
（「TASK-12.4 第三者検証のモデル多様性制約を恒久追跡する」）の成果物。本書は制約解消までの
**恒久追跡**のみを目的とし、別ベンダー LLM・人間被験の**実施そのもの**は行わない（実施経路
の未定義は既存判定記録どおり「不可・要エスカレーション」のまま、2 節参照）。

## 1. 背景・経緯

1. **PoC-9**（`docs/spec/03-poc/ai-first-maintainability/README.md`）: タスク設計者・被験
   AI・評価者を単一エージェント（同一セッション）が兼務するセルフ実験であり、自己評価
   バイアスを排除できていない。
2. **TASK-12.4 3 役分離**（#85／#86、`docs/design/third-party-verification.md`・
   `docs/design/third-party-feasibility-verification.md`）: 3 役を分離するプロトコルを
   設計し、「別セッション・可能であれば別モデル」を要求仕様とした。
3. **実測定**（`docs/reports/task-12-4-1-completion-rate-verification.md`・
   `task-12-4-2-feasibility-judgment-verification.md`）: 調整・評価役を `Claude Fable 5`、
   被験 AI を `claude-sonnet-5` ×10（別セッション）として実施。「別セッション」「別 Claude
   モデル」は充足したが、**Claude ファミリー内**に留まり、別ベンダー LLM・人間被験による
   追検証は実施できなかった。
4. **イシュー #241 → PR #247**（コミット `cb60e6c`）: この限界を `docs/reports/
   task-12-4-third-party-scope-feasibility.md` として「不可・要エスカレーション（未定義
   依存）」に判定・記録し、`third-party-verification.md`・`third-party-feasibility-
   verification.md`・両実測定レポートへ「被験構成の実態と限界」節を追記してサインオフを
   実態化した。以降 TASK-12.5（#46）・TASK-12.6（#47）・TASK-12.7（#48）の実測定も同一の
   Claude ファミリー内構成で実施され、いずれも「既知の限界」としてこの制約を継承している。
5. **イシュー #252（仕様照合）**: #241 の恒久追跡先が**クローズ済みイシュー #241 のみ**で
   あり、open な追跡が存在しないことを検出。
6. **本書 + イシュー #262**: #262 を制約解消までの open な恒久追跡先とし、再検証が可能に
   なった時点の実施条件（3 節）と現行サインオフの有効範囲（2 節）を整理する。

## 2. 現行サインオフの有効範囲（暫定運用）

### 2.1 有効範囲

現行サインオフ（TASK-12.4／12.5／12.6／12.7 の確定値・Conditional Go 条件 (3) の充足判断）
は、**「Claude ファミリー内での別セッション・別モデルによる 3 役分離検証」の範囲でのみ
有効**である。実施済み構成の実態:

| 項目 | 実態 |
|------|------|
| 調整・評価役 | `Claude Fable 5`（別セッション） |
| 被験 AI | `claude-sonnet-5` ×10（タスクごとに独立セッション） |
| 完遂率（TASK-12.4-1） | 8/10 |
| 可否判定正解率（TASK-12.4-2） | 8/10（初回）、グレーゾーン v2（TASK-12.6）は 9/10 |
| ベンダー多様性 | なし（Anthropic Claude ファミリー内のみ） |
| 人間被験 | なし |

この範囲を超えて「別ベンダー LLM・人間被験によっても同水準の結果が得られる」という一般化
した主張はしない。`docs/acceptance/req12-ai-autonomy.md`・`docs/reports/
task-12-7-acceptance.md` の「既知の限界」節が明記するとおり、被験 AI は Claude ファミリーに
限られるという限定つきの確定値である。

### 2.2 暫定運用

- 制約解消（3 節の実施条件充足）まで、**新規の第三者検証も現行と同一の Claude ファミリー内
  構成で実施してよい**（実施不能な依存を待って検証自体を止めない）。
- ただしその場合、レポート（`docs/reports/task-12-*.md`）へ「既知の限界」として本制約と
  イシュー #262 への参照を必ず記載する（`.claude/rules/security.md` の「握りつぶし禁止」・
  `.claude/rules/improvement-proposal.md` のフェイルクローズ原則と同一趣旨）。
- **限界受容の最終判断は人間に留保する**。AI 実装セッションが「この限界は無視してよい」
  「別モデル要求は事実上満たされたとみなす」と自己確定しない。要人間判断事項は
  `docs/reports/task-12-4-third-party-scope-feasibility.md`「要人間判断事項」1〜3 を正とし、
  本書で再定義しない。

## 3. 再検証の実施条件

制約解消（別ベンダー LLM・人間被験による再検証）に必要な条件を、[`feasibility-
guardrail.md`](./feasibility-guardrail.md) の 3 軸（実施可能か・安全か・影響範囲が許容内
か）のうち現状不充足な「実施可能か」軸を満たすために必要な依存として整理する。

### 3.1 別ベンダー LLM 被験の場合

- **必要な環境**: 別ベンダー LLM（Claude 系以外）の呼び出し手段（API アクセス・エージェント
  ハーネス・使い捨て worktree へのタスク文面注入手段）。具体的な API キー・接続情報・
  アカウント契約の内容は本書に記載しない（`task-12-4-third-party-scope-feasibility.md` の
  未定義依存カテゴリと同一運用。2 節参照）。
- **被験対象**: 新規タスク設計は不要。コミット固定済みの既存タスク定義（TASK-12.4-1 の
  T-01〜T-10、TASK-12.4-2 の J-01〜J-10、TASK-12.6 の G-01〜G-10 v2）をそのまま再利用する
  （後出し防止の原則を維持するため、被験モデルを差し替える場合もタスク文面・正解ラベルは
  変更しない）。
- **判定手順**: `scripts/third-party-verify.sh`（完遂率）・`scripts/third-party-
  feasibility-verify.sh`（可否判定正解率）と、[`ci-completion-criteria.md`](./ci-completion-criteria.md)
  準拠の機械ゲートをそのまま再利用する。算出した完遂率・正解率を REQ-12 の閾値
  （完遂率 60% 以上・正解率 80% 以上）と対比し、既存の Claude ファミリー内実測値と並記した
  比較表として記録する。

### 3.2 人間被験の場合

- **必要な環境**: 既存の自己サインオフアカウント（`aLiz-Nancy` 等）以外の、実在の外部人間
  被験者を確保するプロセス（募集・同意取得・報酬等の運用設計。具体的な人選・契約内容は
  本書のスコープ外）。
- **被験対象・判定手順**: 3.1 と同一のタスク文面・機械ゲートを用いる。承認は GitHub review
  approval イベントで裏付ける（#247 で確立した「AI 実装/マージセッションが記録した自己
  サインオフ」と「独立した外部人間ユーザーによる review 承認」の区別運用をそのまま踏襲する）。

### 3.3 着手時の手順

いずれのケースも、依存（呼び出し手段・被験者確保プロセス）が具体的に定義された時点で
[`feasibility-guardrail.md`](./feasibility-guardrail.md) の 3 軸再判定を行い、`docs/reports/
task-12-4-third-party-scope-feasibility.md` が記録した「未定義依存」該当箇所が解消された
ことを確認してから着手する。3 軸すべて充足を確認せずに着手しない（fail-closed、
`.claude/rules/feasibility-guardrail.md`「条件付き可の扱い」と同一原則）。

## 4. 追跡運用

- **イシュー #262 を、本制約が解消されるまでの open な恒久追跡先とする。** 関連文書
  （5 節）から本書とイシュー #262 の双方を参照することで、クローズ済みイシューのみに
  依存する状態（#252 が検出した問題）の再発を防ぐ。
- **クローズ条件**（いずれか）:
  - (a) 別ベンダー LLM または人間被験による再検証が実施完了し、結果がレポートへ反映され
    人間レビューの承認を得たとき。
  - (b) 人間による限界受容の確定判断が記録されたとき（`task-12-4-third-party-scope-
    feasibility.md`「要人間判断事項」1 の decision）。
- **フェイルセーフ**: 万一 #262 が上記条件を満たさないまま先にクローズされた場合は、
  後継の open イシューへ追跡を引き継ぎ、本書および 5 節の関連文書の参照を新イシュー番号へ
  更新する。追跡の断絶を防ぐため、クローズ時は必ず後継先の有無を確認する運用とする。

## 5. 関連ドキュメント

- 3 役分離プロトコル（完遂率）: [`third-party-verification.md`](./third-party-verification.md)
- 3 役分離プロトコル（可否判定正解率）: [`third-party-feasibility-verification.md`](./third-party-feasibility-verification.md)
- グレーゾーン再検証プロトコル: [`gray-zone-feasibility-verification.md`](./gray-zone-feasibility-verification.md)
- 判定基準の定義: [`feasibility-guardrail.md`](./feasibility-guardrail.md)
- CI 完遂判定基準: [`ci-completion-criteria.md`](./ci-completion-criteria.md)
- 対応可否判定記録（イシュー #241 対応、判定区分: 不可・要エスカレーション）:
  [`../reports/task-12-4-third-party-scope-feasibility.md`](../reports/task-12-4-third-party-scope-feasibility.md)
- 完遂率実測定レポート: [`../reports/task-12-4-1-completion-rate-verification.md`](../reports/task-12-4-1-completion-rate-verification.md)
- 可否判定正解率実測定レポート: [`../reports/task-12-4-2-feasibility-judgment-verification.md`](../reports/task-12-4-2-feasibility-judgment-verification.md)
- グレーゾーン実測定レポート: [`../reports/task-12-6-gray-zone-verification.md`](../reports/task-12-6-gray-zone-verification.md)
- TASK-12.7 確定値受け入れレポート: [`../reports/task-12-7-acceptance.md`](../reports/task-12-7-acceptance.md)
- REQ-12 受け入れ検証レポート: [`../acceptance/req12-ai-autonomy.md`](../acceptance/req12-ai-autonomy.md)
- 対応可否自律判断ガードレール規約: [`../../.claude/rules/feasibility-guardrail.md`](../../.claude/rules/feasibility-guardrail.md)
- セキュリティ規約: [`../../.claude/rules/security.md`](../../.claude/rules/security.md)
- 根拠要件: `docs/spec/04-requirements.md` REQ-12・Conditional Go 条件 (3)
- 恒久追跡先: イシュー #262（本書の起票元。クローズ条件は 4 節）
