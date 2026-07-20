//! 利用者アプリ独自の OpenAPI スキーマ登録（イシュー #320）。
//!
//! # 役割・責務境界
//!
//! `crates/plugin-openapi` の他モジュール（[`crate::docs::ApiDoc`] /
//! [`crate::embed`]）はフレームワーク自身の 5 エンドポイントに固定された
//! コンパイル時埋め込みを扱う。本モジュールはそれとは独立に、**利用者アプリ**
//! が自前で生成した OpenAPI ドキュメント（バイト列）を実行時に登録するための
//! 薄いコンテナ [`OpenApiDoc`] を提供する。`_/todo-backend` 等、
//! `fandhe_backend_routes::Router` を独自ルーティングに使う利用者アプリが、
//! `Server::openapi()`（[`crate::OPENAPI_JSON`] 固定配信）の代わりに自前の
//! `utoipa::OpenApi` 実装や他ツールで生成した JSON を `GET /openapi.json` /
//! `GET /openapi.yaml` として配信できるようにする（Issue #320 の受け入れ
//! 基準）。
//!
//! # 接続契約（`crates/core` との関係）
//!
//! `crates/core::Server::openapi_with(doc)`（`openapi` feature 限定 API）が
//! [`OpenApiDoc`] を受け取り、`crate::plugin::try_intercept` が
//! `GET /openapi.json` / `GET /openapi.yaml` へ配信する。`Server::openapi()`
//! （フレームワーク固定の [`crate::OPENAPI_JSON`]）と `Server::openapi_with`
//! は排他ではなく後勝ち（`crates/core/src/server.rs` の `OpenApiRegistration`
//! doc を参照）。
//!
//! # 検証タイミング（fail-closed）
//!
//! [`OpenApiDoc::from_json`] は構築時（= 利用者アプリの起動シーケンス内、
//! `Server::openapi_with` 呼び出し時点）に JSON 妥当性を一度だけ検証する。
//! 構築に成功した [`OpenApiDoc`] は「検証済み」であることが型で保証され、
//! 以降のリクエスト処理経路（`try_intercept`）では再検証しない（実行時
//! コストを増やさない、PoC-4 成功基準 3 と同じ設計判断）。不正な JSON は
//! `Err` で伝播し、ライブラリ境界を越えて panic させない
//! （`.claude/rules/coding-rust.md`）。
//!
//! YAML は意味的検証を行わない（非空・UTF-8 のみ）。サーバ経路へ YAML
//! パーサ依存（`utoipa/yaml` の `serde_norway`）を持ち込まない #279 の設計
//! 判断を維持するため、YAML 表現は利用者が事前生成したバイト列をそのまま
//! 信頼する契約とする。

use std::fmt;

/// [`OpenApiDoc::from_json`] / [`OpenApiDoc::with_yaml`] が返しうるエラー。
///
/// `crates/plugin-websocket/src/error.rs::WsError` と同型（手書き enum +
/// `Display` + `std::error::Error`、thiserror 依存を追加しない）。
#[derive(Debug)]
pub enum OpenApiDocError {
    /// `from_json` に渡したバイト列が JSON として構文的に不正
    /// （`serde_json::from_slice` が失敗した）。
    InvalidJson(serde_json::Error),
    /// `from_json` に渡した JSON がトップレベルでオブジェクトでない
    /// （OpenAPI ドキュメントは `{"openapi": ..., "info": ..., ...}` の
    /// オブジェクトである必要がある。配列・文字列・数値等は拒否する）。
    NotAnObject,
    /// `with_yaml` に渡したバイト列が空、または妥当な UTF-8 でない。
    InvalidYaml,
}

impl fmt::Display for OpenApiDocError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OpenApiDocError::InvalidJson(_) => {
                write!(f, "openapi doc は妥当な JSON ではない")
            }
            OpenApiDocError::NotAnObject => {
                write!(
                    f,
                    "openapi doc の JSON トップレベルはオブジェクトである必要がある"
                )
            }
            OpenApiDocError::InvalidYaml => {
                write!(f, "openapi doc の yaml 表現は非空の UTF-8 である必要がある")
            }
        }
    }
}

impl std::error::Error for OpenApiDocError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            OpenApiDocError::InvalidJson(e) => Some(e),
            OpenApiDocError::NotAnObject | OpenApiDocError::InvalidYaml => None,
        }
    }
}

/// 利用者アプリ独自の OpenAPI ドキュメント（検証済みバイト列のコンテナ）。
///
/// `from_json` の構築成功が「JSON として妥当かつトップレベルがオブジェクト」
/// であることの検証済みを意味する（型による保証、fail-closed）。
/// `crates/core::Server::openapi_with` に登録して使う。
///
/// # Examples
/// ```
/// use fandhe_backend_plugin_openapi::OpenApiDoc;
///
/// let doc = OpenApiDoc::from_json(r#"{"openapi":"3.0.0","info":{"title":"t","version":"1"}}"#)
///     .expect("妥当な JSON");
/// assert!(doc.json().starts_with(b"{"));
/// assert!(doc.yaml().is_none());
/// ```
#[derive(Debug, Clone)]
pub struct OpenApiDoc {
    json: Vec<u8>,
    yaml: Option<Vec<u8>>,
}

impl OpenApiDoc {
    /// JSON バイト列を検証して [`OpenApiDoc`] を構築する。
    ///
    /// 検証は「`serde_json::from_slice` でパース可能」かつ「トップレベルが
    /// オブジェクト」の 2 点のみ（OpenAPI 3.x スキーマ準拠の意味的検証は
    /// スコープ外。フレームワーク自身の [`crate::ApiDoc`] 側でも TASK-3.3・
    /// #32 の未了スコープと同一）。構築後は元の `json` バイト列をそのまま
    /// 保持し、パース結果を破棄する（レスポンスは常に利用者が渡したバイト列
    /// そのものを返す契約、`crates/core/src/plugin.rs` の doc を参照）。
    ///
    /// # Errors
    /// JSON として構文的に不正な場合は [`OpenApiDocError::InvalidJson`]、
    /// トップレベルがオブジェクトでない場合は [`OpenApiDocError::NotAnObject`]
    /// を返す。
    ///
    /// # Examples
    /// ```
    /// use fandhe_backend_plugin_openapi::OpenApiDoc;
    ///
    /// // 構文不正 → Err（fail-closed）。
    /// assert!(OpenApiDoc::from_json("{not json").is_err());
    /// // トップレベルが配列 → Err。
    /// assert!(OpenApiDoc::from_json("[1, 2, 3]").is_err());
    /// // 妥当な JSON → Ok。
    /// assert!(OpenApiDoc::from_json(r#"{"openapi":"3.0.0"}"#).is_ok());
    /// ```
    pub fn from_json(json: impl Into<Vec<u8>>) -> Result<Self, OpenApiDocError> {
        let json = json.into();
        let value: serde_json::Value =
            serde_json::from_slice(&json).map_err(OpenApiDocError::InvalidJson)?;
        if !value.is_object() {
            return Err(OpenApiDocError::NotAnObject);
        }
        Ok(Self { json, yaml: None })
    }

    /// 事前生成済みの YAML 表現を任意登録する（`GET /openapi.yaml` 用）。
    ///
    /// 意味的検証は行わない（非空・UTF-8 のみ、モジュール doc を参照）。
    /// 未呼び出しの場合、`GET /openapi.yaml` は既定 `Handler` へフォール
    /// スルーする（`crates/core/src/plugin.rs` の `try_intercept` doc を参照）。
    ///
    /// # Errors
    /// バイト列が空、または妥当な UTF-8 でない場合は
    /// [`OpenApiDocError::InvalidYaml`] を返す。
    ///
    /// # Examples
    /// ```
    /// use fandhe_backend_plugin_openapi::OpenApiDoc;
    ///
    /// let doc = OpenApiDoc::from_json(r#"{"openapi":"3.0.0"}"#)
    ///     .expect("妥当な JSON")
    ///     .with_yaml("openapi: 3.0.0\n")
    ///     .expect("妥当な yaml バイト列");
    /// assert_eq!(doc.yaml(), Some(b"openapi: 3.0.0\n".as_slice()));
    ///
    /// // 空バイト列は Err。
    /// let err = OpenApiDoc::from_json(r#"{"openapi":"3.0.0"}"#)
    ///     .expect("妥当な JSON")
    ///     .with_yaml(Vec::new());
    /// assert!(err.is_err());
    /// ```
    pub fn with_yaml(mut self, yaml: impl Into<Vec<u8>>) -> Result<Self, OpenApiDocError> {
        let yaml = yaml.into();
        if yaml.is_empty() || std::str::from_utf8(&yaml).is_err() {
            return Err(OpenApiDocError::InvalidYaml);
        }
        self.yaml = Some(yaml);
        Ok(self)
    }

    /// 登録済み JSON バイト列を返す（`GET /openapi.json` のレスポンス body）。
    #[must_use]
    pub fn json(&self) -> &[u8] {
        &self.json
    }

    /// 登録済み YAML バイト列を返す（`with_yaml` 未呼び出しなら `None`）。
    #[must_use]
    pub fn yaml(&self) -> Option<&[u8]> {
        self.yaml.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::{OpenApiDoc, OpenApiDocError};

    #[test]
    fn from_json_accepts_valid_object() {
        let doc = OpenApiDoc::from_json(r#"{"openapi":"3.0.0"}"#).unwrap();
        assert_eq!(doc.json(), br#"{"openapi":"3.0.0"}"#);
        assert!(doc.yaml().is_none());
    }

    #[test]
    fn from_json_rejects_syntactically_invalid_json() {
        let err = OpenApiDoc::from_json("{not json").unwrap_err();
        assert!(matches!(err, OpenApiDocError::InvalidJson(_)));
    }

    #[test]
    fn from_json_rejects_non_object_top_level() {
        assert!(matches!(
            OpenApiDoc::from_json("[1, 2, 3]").unwrap_err(),
            OpenApiDocError::NotAnObject
        ));
        assert!(matches!(
            OpenApiDoc::from_json("\"just a string\"").unwrap_err(),
            OpenApiDocError::NotAnObject
        ));
        assert!(matches!(
            OpenApiDoc::from_json("42").unwrap_err(),
            OpenApiDocError::NotAnObject
        ));
    }

    #[test]
    fn with_yaml_accepts_non_empty_utf8() {
        let doc = OpenApiDoc::from_json(r#"{"openapi":"3.0.0"}"#)
            .unwrap()
            .with_yaml("openapi: 3.0.0\n")
            .unwrap();
        assert_eq!(doc.yaml(), Some(b"openapi: 3.0.0\n".as_slice()));
    }

    #[test]
    fn with_yaml_rejects_empty_bytes() {
        let err = OpenApiDoc::from_json(r#"{"openapi":"3.0.0"}"#)
            .unwrap()
            .with_yaml(Vec::new())
            .unwrap_err();
        assert!(matches!(err, OpenApiDocError::InvalidYaml));
    }

    #[test]
    fn with_yaml_rejects_non_utf8_bytes() {
        let err = OpenApiDoc::from_json(r#"{"openapi":"3.0.0"}"#)
            .unwrap()
            .with_yaml(vec![0xff, 0xfe, 0xfd])
            .unwrap_err();
        assert!(matches!(err, OpenApiDocError::InvalidYaml));
    }

    /// 登録前に複製・比較しやすいことを確認する（`Clone` の補助テスト）。
    #[test]
    fn open_api_doc_is_cloneable() {
        let doc = OpenApiDoc::from_json(r#"{"openapi":"3.0.0"}"#).unwrap();
        let cloned = doc.clone();
        assert_eq!(doc.json(), cloned.json());
    }
}
