//! `gen-openapi` — [`fandhe_backend_plugin_openapi::ApiDoc`] を JSON/YAML にシリアライズし
//! `crates/plugin-openapi/openapi.json` / `openapi.yaml`（`embed.rs` が `include_str!` で
//! 埋め込む実体）を生成・検証する開発用 CLI（TASK-3.2、#31、REQ-3【Must】。YAML 対応は
//! #279、仕様が明記する「GET /openapi.yaml も同等に提供」の解消）。
//!
//! # 呼び出し元・実行タイミング
//! - **開発者がローカルで再生成する場合**: 既定（引数なし）または `--update` で実行し
//!   `openapi.json` / `openapi.yaml` を両方上書きする。
//! - **CI（`scripts/openapi-two-stage.sh` stage 1）**: `--check` で実行し、コミット済み
//!   `openapi.json` / `openapi.yaml` が [`ApiDoc`] の最新定義から生成した内容と一致するかを
//!   両方検証する。乖離（`ApiDoc` を変更したが再生成を忘れた等）を非 0 終了で検知する
//!   fail-closed ゲート（`.claude/rules/security.md` A08 対策）。JSON/YAML は同一
//!   [`ApiDoc`] を単一のスキーマ源とするため、どちらか一方だけの再生成漏れも検知する。
//!
//! # feature 前提（pay-for-what-you-use）
//! 本バイナリは `gen-cli` feature（`required-features`）と `utoipa/yaml`
//! （serde_norway 経由）を必要とする。`serde_json` はイシュー #320
//! （`custom.rs::OpenApiDoc::from_json`）で通常依存へ変更済みのため
//! `gen-cli` 無効時にも本クレートには残るが、`utoipa` が推移的に引き込む
//! ため `cargo tree` 上の推移依存差はゼロ（`Cargo.toml` の doc comment を
//! 参照）。`fandhe-backend-plugin-openapi` をサーバ側から lib として参照する
//! 通常経路（`gen-cli` 無効時）には、本ファイル・serde_norway は一切ビルド
//! 対象に含まれない（`.claude/rules/pay-for-what-you-use.md`）。
//!
//! # 引数
//! - （引数なし）: `openapi.json` / `openapi.yaml` を生成し既定の出力先へ書き込む
//! - `--update`: 既定と同じ（生成 + 書き込み）。CI ステップ名との対比で意図を明示する用途
//! - `--check`: 書き込まず、生成結果と既存ファイル（json/yaml 両方）を比較する。
//!   差分があれば非 0 終了
//! - `--output <path>`: JSON の出力・比較対象のパスを既定
//!   （`$CARGO_MANIFEST_DIR/openapi.json`）から変更する
//! - `--output-yaml <path>`: YAML の出力・比較対象のパスを既定
//!   （`$CARGO_MANIFEST_DIR/openapi.yaml`）から変更する
//!
//! 未知の引数はフェイルクローズで usage を表示し非 0 終了する
//! （OWASP A03 対策、固定引数のみ受け付け、`.claude/rules/security.md`）。

use fandhe_backend_plugin_openapi::{ApiDoc, OpenApi};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// リポジトリにコミットする JSON 生成物の既定パス（`crates/plugin-openapi/openapi.json`）。
/// `CARGO_MANIFEST_DIR` は本クレートのルートを指すため `cargo run` の起動元ディレクトリに
/// 依存しない（CI・ローカル両方で同一パスに解決される）。
fn default_output_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("openapi.json")
}

/// リポジトリにコミットする YAML 生成物の既定パス（`crates/plugin-openapi/openapi.yaml`）。
fn default_output_yaml_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("openapi.yaml")
}

/// [`ApiDoc::openapi()`] から pretty JSON（末尾改行付き）を生成する。
///
/// `openapi.json`（`embed.rs` の `include_str!` 対象）とテスト（`embed.rs` の鮮度検証）が
/// 同一表現を共有できるよう、シリアライズ処理は本関数に集約する。
fn generate_json() -> String {
    // utoipa::OpenApi::openapi() はコンパイル時に構築されたメタデータから実行時に
    // ドキュメント構造体を組み立てるのみで失敗し得ないが、シリアライズ自体は
    // serde_json 経由で失敗しうる（循環参照等）ため Result を伝播する。
    let mut json = ApiDoc::openapi()
        .to_pretty_json()
        .expect("ApiDoc の JSON シリアライズに失敗した（utoipa 内部の serde_json エラー）");
    if !json.ends_with('\n') {
        json.push('\n');
    }
    json
}

/// [`ApiDoc::openapi()`] から YAML（末尾改行付き）を生成する。
///
/// JSON と同一の [`ApiDoc`] を単一のスキーマ源とすることで、`GET /openapi.json` と
/// `GET /openapi.yaml` の内容乖離を構造的に排除する（`embed.rs` の鮮度テストが両者を
/// 個別に検証する前提）。`to_yaml()` は `utoipa/yaml` feature（serde_norway 経由）が
/// 必要（本ファイルは `gen-cli` feature 限定でビルドされるため常に有効）。
fn generate_yaml() -> String {
    let mut yaml = ApiDoc::openapi()
        .to_yaml()
        .expect("ApiDoc の YAML シリアライズに失敗した（utoipa 内部の serde_norway エラー）");
    if !yaml.ends_with('\n') {
        yaml.push('\n');
    }
    yaml
}

enum Mode {
    Write,
    Check,
}

/// 解析済み CLI 引数（モード + JSON/YAML それぞれの出力先パス）。
struct Args {
    mode: Mode,
    output_json: PathBuf,
    output_yaml: PathBuf,
}

fn parse_args(args: &[String]) -> Result<Args, String> {
    let mut mode = Mode::Write;
    let mut output_json = default_output_path();
    let mut output_yaml = default_output_yaml_path();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--check" => mode = Mode::Check,
            "--update" => mode = Mode::Write,
            "--output" => {
                let path = iter.next().ok_or_else(|| {
                    "--output には値が必要（例: --output /path/to/openapi.json）".to_string()
                })?;
                output_json = PathBuf::from(path);
            }
            "--output-yaml" => {
                let path = iter.next().ok_or_else(|| {
                    "--output-yaml には値が必要（例: --output-yaml /path/to/openapi.yaml）"
                        .to_string()
                })?;
                output_yaml = PathBuf::from(path);
            }
            other => {
                return Err(format!(
                    "未知の引数: {other}\nusage: gen-openapi [--check | --update] \
                     [--output <path>] [--output-yaml <path>]"
                ));
            }
        }
    }
    Ok(Args {
        mode,
        output_json,
        output_yaml,
    })
}

/// 生成結果 1 件を書き込む・または既存ファイルとの一致を検証する（json/yaml 共通処理）。
/// `--check` で乖離を検知した場合、呼び出し元は非 0 終了する（fail-closed）。
fn write_or_check(mode: &Mode, output: &Path, generated: &str, label: &str) -> bool {
    match mode {
        Mode::Write => match fs::write(output, generated) {
            Ok(()) => {
                println!("{label} を生成した: {}", output.display());
                true
            }
            Err(err) => {
                eprintln!("{} への書き込みに失敗した: {err}", output.display());
                false
            }
        },
        Mode::Check => match fs::read_to_string(output) {
            Ok(existing) if existing == generated => {
                println!("{label} は最新（{}）", output.display());
                true
            }
            Ok(_) => {
                eprintln!(
                    "{} が ApiDoc の最新定義と乖離している。`cargo run -p fandhe-backend-plugin-openapi \
                     --features gen-cli --bin gen-openapi -- --update` で再生成すること",
                    output.display()
                );
                false
            }
            Err(err) => {
                eprintln!("{} の読み込みに失敗した: {err}", output.display());
                false
            }
        },
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let parsed = match parse_args(&args) {
        Ok(v) => v,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };

    // JSON と YAML は同一 ApiDoc を単一のスキーマ源とするため、`--check` は
    // 両方を検証しどちらか一方でも乖離していれば非 0 終了する（fail-closed、
    // `.claude/rules/security.md` A08。どちらか片方だけの再生成漏れも検知する）。
    let json_ok = write_or_check(
        &parsed.mode,
        &parsed.output_json,
        &generate_json(),
        "openapi.json",
    );
    let yaml_ok = write_or_check(
        &parsed.mode,
        &parsed.output_yaml,
        &generate_yaml(),
        "openapi.yaml",
    );

    if json_ok && yaml_ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_defaults_to_write_and_default_output() {
        let parsed = parse_args(&[]).expect("空引数は解析できる");
        assert!(matches!(parsed.mode, Mode::Write));
        assert_eq!(parsed.output_json, default_output_path());
        assert_eq!(parsed.output_yaml, default_output_yaml_path());
    }

    #[test]
    fn parse_args_recognizes_check_flag() {
        let args = vec!["--check".to_string()];
        let parsed = parse_args(&args).expect("--check は解析できる");
        assert!(matches!(parsed.mode, Mode::Check));
    }

    #[test]
    fn parse_args_recognizes_output_override() {
        let args = vec!["--output".to_string(), "/tmp/custom.json".to_string()];
        let parsed = parse_args(&args).expect("--output は解析できる");
        assert_eq!(parsed.output_json, PathBuf::from("/tmp/custom.json"));
    }

    #[test]
    fn parse_args_recognizes_output_yaml_override() {
        let args = vec!["--output-yaml".to_string(), "/tmp/custom.yaml".to_string()];
        let parsed = parse_args(&args).expect("--output-yaml は解析できる");
        assert_eq!(parsed.output_yaml, PathBuf::from("/tmp/custom.yaml"));
    }

    #[test]
    fn parse_args_rejects_unknown_flag() {
        let args = vec!["--bogus".to_string()];
        assert!(
            parse_args(&args).is_err(),
            "未知の引数はフェイルクローズで拒否する"
        );
    }

    #[test]
    fn parse_args_rejects_output_without_value() {
        let args = vec!["--output".to_string()];
        assert!(
            parse_args(&args).is_err(),
            "--output に値がない場合はフェイルクローズで拒否する"
        );
    }

    #[test]
    fn parse_args_rejects_output_yaml_without_value() {
        let args = vec!["--output-yaml".to_string()];
        assert!(
            parse_args(&args).is_err(),
            "--output-yaml に値がない場合はフェイルクローズで拒否する"
        );
    }

    #[test]
    fn generate_json_is_reparsable_and_ends_with_newline() {
        let json = generate_json();
        assert!(json.ends_with('\n'));
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("生成された JSON の再パースに失敗した");
        assert_eq!(parsed["paths"].as_object().unwrap().len(), 5);
    }

    #[test]
    fn generate_yaml_is_reparsable_and_ends_with_newline() {
        let yaml = generate_yaml();
        assert!(yaml.ends_with('\n'));
        assert!(yaml.starts_with("openapi:"));
        // JSON と同一 ApiDoc を単一のスキーマ源とするため、YAML 側も
        // `paths:` 直下のトップレベルエントリ数（インデント 2 + `/` 始まり）が
        // JSON 側の想定（5 件）と一致することを確認する。
        let paths_entry_count = yaml.lines().filter(|line| line.starts_with("  /")).count();
        assert_eq!(paths_entry_count, 5);
    }
}
