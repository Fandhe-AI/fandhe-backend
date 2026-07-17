//! `gen-openapi` — [`bf_plugin_openapi::ApiDoc`] を JSON にシリアライズし
//! `crates/plugin-openapi/openapi.json`（`embed.rs` が `include_str!` で埋め込む実体）
//! を生成・検証する開発用 CLI（TASK-3.2、#31、REQ-3【Must】）。
//!
//! # 呼び出し元・実行タイミング
//! - **開発者がローカルで再生成する場合**: 既定（引数なし）または `--update` で実行し
//!   `openapi.json` を上書きする。
//! - **CI（`scripts/openapi-two-stage.sh` stage 1）**: `--check` で実行し、コミット済み
//!   `openapi.json` が [`ApiDoc`] の最新定義から生成した内容と一致するかを検証する。
//!   乖離（`ApiDoc` を変更したが `openapi.json` の再生成を忘れた等）を非 0 終了で検知する
//!   fail-closed ゲート（`.claude/rules/security.md` A08 対策）。
//!
//! # feature 前提（pay-for-what-you-use）
//! 本バイナリは `gen-cli` feature（`required-features`）と `dep:serde_json` を必要とする。
//! `bf-plugin-openapi` をサーバ側から lib として参照する通常経路（`gen-cli` 無効時）には
//! 本ファイル・`serde_json` は一切ビルド対象に含まれない
//! （`.claude/rules/pay-for-what-you-use.md`）。
//!
//! # 引数
//! - （引数なし）: `openapi.json` を生成し既定の出力先へ書き込む
//! - `--update`: 既定と同じ（生成 + 書き込み）。CI ステップ名との対比で意図を明示する用途
//! - `--check`: 書き込まず、生成結果と既存ファイルを比較する。差分があれば非 0 終了
//! - `--output <path>`: 出力・比較対象のパスを既定
//!   （`$CARGO_MANIFEST_DIR/openapi.json`）から変更する
//!
//! 未知の引数はフェイルクローズで usage を表示し非 0 終了する
//! （OWASP A03 対策、固定引数のみ受け付け、`.claude/rules/security.md`）。

use bf_plugin_openapi::{ApiDoc, OpenApi};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// リポジトリにコミットする生成物の既定パス（`crates/plugin-openapi/openapi.json`）。
/// `CARGO_MANIFEST_DIR` は本クレートのルートを指すため `cargo run` の起動元ディレクトリに
/// 依存しない（CI・ローカル両方で同一パスに解決される）。
fn default_output_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("openapi.json")
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

enum Mode {
    Write,
    Check,
}

fn parse_args(args: &[String]) -> Result<(Mode, PathBuf), String> {
    let mut mode = Mode::Write;
    let mut output = default_output_path();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--check" => mode = Mode::Check,
            "--update" => mode = Mode::Write,
            "--output" => {
                let path = iter.next().ok_or_else(|| {
                    "--output には値が必要（例: --output /path/to/openapi.json）".to_string()
                })?;
                output = PathBuf::from(path);
            }
            other => {
                return Err(format!(
                    "未知の引数: {other}\nusage: gen-openapi [--check | --update] [--output <path>]"
                ));
            }
        }
    }
    Ok((mode, output))
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mode, output) = match parse_args(&args) {
        Ok(v) => v,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };

    let generated = generate_json();

    match mode {
        Mode::Write => match fs::write(&output, &generated) {
            Ok(()) => {
                println!("openapi.json を生成した: {}", output.display());
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("{} への書き込みに失敗した: {err}", output.display());
                ExitCode::FAILURE
            }
        },
        Mode::Check => match fs::read_to_string(&output) {
            Ok(existing) if existing == generated => {
                println!("openapi.json は最新（{}）", output.display());
                ExitCode::SUCCESS
            }
            Ok(_) => {
                eprintln!(
                    "{} が ApiDoc の最新定義と乖離している。`cargo run -p bf-plugin-openapi \
                     --features gen-cli --bin gen-openapi -- --update` で再生成すること",
                    output.display()
                );
                ExitCode::FAILURE
            }
            Err(err) => {
                eprintln!("{} の読み込みに失敗した: {err}", output.display());
                ExitCode::FAILURE
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_defaults_to_write_and_default_output() {
        let (mode, output) = parse_args(&[]).expect("空引数は解析できる");
        assert!(matches!(mode, Mode::Write));
        assert_eq!(output, default_output_path());
    }

    #[test]
    fn parse_args_recognizes_check_flag() {
        let args = vec!["--check".to_string()];
        let (mode, _) = parse_args(&args).expect("--check は解析できる");
        assert!(matches!(mode, Mode::Check));
    }

    #[test]
    fn parse_args_recognizes_output_override() {
        let args = vec!["--output".to_string(), "/tmp/custom.json".to_string()];
        let (_, output) = parse_args(&args).expect("--output は解析できる");
        assert_eq!(output, PathBuf::from("/tmp/custom.json"));
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
    fn generate_json_is_reparsable_and_ends_with_newline() {
        let json = generate_json();
        assert!(json.ends_with('\n'));
        let parsed: serde_json::Value =
            serde_json::from_str(&json).expect("生成された JSON の再パースに失敗した");
        assert_eq!(parsed["paths"].as_object().unwrap().len(), 5);
    }
}
