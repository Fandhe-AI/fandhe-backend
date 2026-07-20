//! レスポンス側 chunked ストリーミング送信の opt-in API（イシュー #319）。
//!
//! [`crate::server::Handler`] の既定メソッド
//! [`crate::server::Handler::handle_streaming`] が返す型 [`StreamingResponse`] と、
//! その producer 側ハンドル [`BodyWriter`] を定義する。`crates/core/src/server.rs`
//! の書き出しループが `handle_streaming` から `Some` を得た場合のみ、通常の
//! `Content-Length` 一括応答（[`fandhe_backend_http::response::Response::serialize`]）
//! の代わりにこの経路（[`fandhe_backend_http::response::Response::serialize_chunked_head`]
//! と [`fandhe_backend_http::chunked::encode_chunk`] /
//! [`fandhe_backend_http::chunked::encode_terminator`] の組み合わせ）を使う。
//! 既存の `Handler::handle` のみを実装した型は本モジュールを一切意識せずに
//! 動作し続ける（後方互換、`.claude/rules/feature-modification.md` の受け入れ基準 2）。
//!
//! # バックプレッシャ（受け入れ基準 3）
//!
//! [`StreamingResponse::channel`] が返す `mpsc::Sender` は bounded
//! （容量は呼び出し元が指定）であり、[`BodyWriter::send`] は容量超過時に
//! `.await` で待機する。受信側（コアの書き出しループ）はソケットへ実際に
//! 書けた分だけチャネルから取り出すため、producer タスクがソケットの処理
//! 速度を追い越して無制限にメモリを積み上げることはない（サーバ側バッファは
//! 高々 `capacity × 1 チャンク分` に有界。`.claude/rules/security.md` の
//! リソース枯渇 DoS 対策）。
//!
//! # 応答完全性（fail-closed）
//!
//! [`BodyWriter::finish`] を呼ばずに producer 側が `drop` された場合、
//! 受信側は「終端が来ないままチャネルが閉じた」ことを検知し、
//! [`fandhe_backend_http::chunked::encode_terminator`] を送出せず接続を
//! クローズする契約（`crates/core/src/server.rs` の書き出しループを参照）。
//! これにより、打ち切られた応答を完全な応答としてクライアント・キャッシュに
//! 誤認させない（RFC 9112 の length 整合性維持）。

use tokio::sync::mpsc;

/// [`StreamingResponse::channel`] の既定チャネル容量。
///
/// [`BodyWriter`] の doc を参照。小さすぎるとチャンクごとの往復コストが
/// 増え、大きすぎるとバックプレッシャの効きが弱まりサーバ側バッファ上限が
/// 緩む。8 は「数チャンク分のパイプライン化を許しつつ上限を小さく保つ」
/// 妥協値として選定した（実測チューニングは将来の課題、
/// `.claude/rules/out-of-scope-tracking.md` 対象候補）。
const DEFAULT_CHANNEL_CAPACITY: usize = 8;

/// [`BodyWriter`] から [`StreamingResponse`] へ送られる 1 メッセージ。
///
/// 非公開。外部からは [`BodyWriter::send`] / [`BodyWriter::finish`] 経由でのみ
/// 生成でき、任意の `StreamEvent` を直接構築させない。
pub(crate) enum StreamEvent {
    /// 1 チャンク分のボディデータ。空データは
    /// [`fandhe_backend_http::chunked::encode_chunk`] 側で無出力になる
    /// （誤終端防止）ため、ここでは特別扱いしない。
    Chunk(Vec<u8>),
    /// 明示的な終端（[`BodyWriter::finish`]）。
    End,
}

/// [`BodyWriter::send`] / [`BodyWriter::finish`] が返すエラー。
///
/// 受信側（コアの書き出しループ）が既に終了した後（クライアント切断・
/// 生存期間上限超過等でソケットを閉じた後）に producer が送信を試みた
/// ことを示す。producer 側はこのエラーを受けたら以降の送信を止め、
/// タスクを終了してよい。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamClosed;

impl std::fmt::Display for StreamClosed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("streaming response receiver has been closed")
    }
}

impl std::error::Error for StreamClosed {}

/// [`crate::server::Handler::handle_streaming`] が返すストリーミング応答。
///
/// ステータス・`Content-Type` のみを保持し、実データは
/// [`StreamingResponse::channel`] で得られる [`BodyWriter`] 経由で producer
/// タスクが逐次送る。`fandhe_backend_http::response::Response` とは異なり
/// body を持たない（chunked framing はコアの書き出しループが直接組み立てる
/// ため、ここでは中間表現としての `Response` を経由しない）。
pub struct StreamingResponse {
    /// 応答ステータスコード。書き出しループが
    /// [`fandhe_backend_http::response::Response::serialize_chunked_head`] へ
    /// そのまま渡す。
    pub status: u16,
    pub(crate) content_type: Option<&'static str>,
    pub(crate) rx: mpsc::Receiver<StreamEvent>,
}

impl StreamingResponse {
    /// `status` の chunked ストリーミング応答と、データ送出用の [`BodyWriter`] を
    /// 既定容量（`DEFAULT_CHANNEL_CAPACITY`）で組み立てる。
    ///
    /// `Content-Type` を付けたい場合は [`Self::channel`] を使う。
    #[must_use]
    pub fn new(status: u16) -> (Self, BodyWriter) {
        Self::channel(status, None, DEFAULT_CHANNEL_CAPACITY)
    }

    /// `status` / `content_type` / 明示的な `capacity`（bounded mpsc の容量）で
    /// ストリーミング応答を組み立てる。
    ///
    /// `capacity` が `0` の場合は `1` に切り上げる（`tokio::sync::mpsc::channel`
    /// は容量 0 を受け付けないため。バックプレッシャの効きを最も強くしたい
    /// 呼び出し元が `0` を渡しても panic させないフェイルセーフ）。
    ///
    /// 戻り値のタプル `(StreamingResponse, BodyWriter)` は、前者を
    /// `Handler::handle_streaming` の戻り値として返し、後者を
    /// `tokio::spawn` した producer タスクへ move して使う想定
    /// （`crate::server::Handler::handle_streaming` の doc test を参照）。
    ///
    /// ```
    /// use fandhe_backend_core::streaming::StreamingResponse;
    ///
    /// # #[tokio::main(flavor = "current_thread")]
    /// # async fn main() {
    /// let (response, writer) = StreamingResponse::channel(200, Some("text/plain"), 4);
    /// assert_eq!(response.status, 200);
    ///
    /// tokio::spawn(async move {
    ///     writer.send(b"hello ".to_vec()).await.ok();
    ///     writer.send(b"world".to_vec()).await.ok();
    ///     writer.finish().await.ok();
    /// });
    /// # let _ = response;
    /// # }
    /// ```
    #[must_use]
    pub fn channel(
        status: u16,
        content_type: Option<&'static str>,
        capacity: usize,
    ) -> (Self, BodyWriter) {
        let (tx, rx) = mpsc::channel(capacity.max(1));
        (
            Self {
                status,
                content_type,
                rx,
            },
            BodyWriter { tx },
        )
    }
}

/// [`StreamingResponse::channel`] の producer 側ハンドル。
///
/// `tokio::spawn` したタスクへ move して使う想定の `Send + 'static` 型。
/// クローンして複数タスクから送信することもできる（`mpsc::Sender` の
/// clone セマンティクスをそのまま継承。最後の 1 つが `finish` するか、
/// 全クローンが `finish` を呼ばずに drop されるとチャネルが閉じる）。
#[derive(Clone)]
pub struct BodyWriter {
    tx: mpsc::Sender<StreamEvent>,
}

impl BodyWriter {
    /// 1 チャンク分のデータを送る。
    ///
    /// チャネルが満杯（[`StreamingResponse::channel`] の `capacity` に到達）の
    /// 場合は受信側（コアの書き出しループ）がソケットへ書き出して空きが
    /// できるまで `.await` で停止する（バックプレッシャ、モジュール冒頭 doc）。
    ///
    /// `data` が空の場合も呼び出し自体は成功するが、送出時に
    /// [`fandhe_backend_http::chunked::encode_chunk`] が無出力にする契約
    /// のため実際のワイヤ出力は生じない（誤終端防止、同関数の doc を参照）。
    ///
    /// # Errors
    ///
    /// 受信側が既に終了している場合は [`StreamClosed`] を返す。
    pub async fn send(&self, data: Vec<u8>) -> Result<(), StreamClosed> {
        self.tx
            .send(StreamEvent::Chunk(data))
            .await
            .map_err(|_| StreamClosed)
    }

    /// ストリームを正常終端する。
    ///
    /// 受信側は [`fandhe_backend_http::chunked::encode_terminator`] を送出し、
    /// 応答を完全なものとして扱う（keep-alive 継続も許される）。`self` を
    /// 消費することで「`finish` 後に `send` を呼ぶ」という無意味な呼び出し
    /// 順序を型レベルで防ぐ。
    ///
    /// `finish` を呼ばずに `self` を drop した場合は打ち切りとして扱われ、
    /// 受信側は終端チャンクを送らず接続をクローズする（モジュール冒頭
    /// doc の「応答完全性」節を参照）。
    ///
    /// # Errors
    ///
    /// 受信側が既に終了している場合は [`StreamClosed`] を返す。
    pub async fn finish(self) -> Result<(), StreamClosed> {
        self.tx
            .send(StreamEvent::End)
            .await
            .map_err(|_| StreamClosed)
    }
}

/// コアの書き出しループ（`crates/core/src/server.rs`）が
/// [`StreamingResponse`] を消費する際の単一ステップの結果。
///
/// `rx.recv()` の戻り値（`Option<StreamEvent>`）をそのまま公開型として
/// 露出させると内部表現（`StreamEvent`）が漏れるため、書き出しループが
/// 使う専用の変換関数 [`StreamingResponse::recv`] を介して受け渡す。
pub(crate) enum RecvOutcome {
    /// 次のチャンクを受信した（空データの可能性を含む）。
    Chunk(Vec<u8>),
    /// producer が [`BodyWriter::finish`] を呼び、正常終端した。
    End,
    /// producer 側が `finish` を呼ばずに drop され、チャネルが閉じた
    /// （打ち切り。書き出しループは終端チャンクを送らず接続を閉じる）。
    Aborted,
}

impl StreamingResponse {
    /// 書き出しループ専用の 1 ステップ受信。`pub(crate)` に限定し、
    /// 外部からは [`StreamingResponse::channel`] で得た [`BodyWriter`] 経由の
    /// 送信のみを許す非対称 API とする（受信は常にコアが握る）。
    pub(crate) async fn recv(&mut self) -> RecvOutcome {
        match self.rx.recv().await {
            Some(StreamEvent::Chunk(data)) => RecvOutcome::Chunk(data),
            Some(StreamEvent::End) => RecvOutcome::End,
            None => RecvOutcome::Aborted,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn send_then_finish_yields_chunks_then_end() {
        let (mut response, writer) = StreamingResponse::channel(200, None, 4);
        writer.send(b"a".to_vec()).await.unwrap();
        writer.send(b"b".to_vec()).await.unwrap();
        writer.finish().await.unwrap();

        match response.recv().await {
            RecvOutcome::Chunk(data) => assert_eq!(data, b"a"),
            _ => panic!("expected first chunk"),
        }
        match response.recv().await {
            RecvOutcome::Chunk(data) => assert_eq!(data, b"b"),
            _ => panic!("expected second chunk"),
        }
        match response.recv().await {
            RecvOutcome::End => {}
            _ => panic!("expected End"),
        }
    }

    #[tokio::test]
    async fn drop_without_finish_yields_aborted() {
        let (mut response, writer) = StreamingResponse::channel(200, None, 4);
        writer.send(b"only".to_vec()).await.unwrap();
        drop(writer);

        match response.recv().await {
            RecvOutcome::Chunk(data) => assert_eq!(data, b"only"),
            _ => panic!("expected chunk before abort"),
        }
        match response.recv().await {
            RecvOutcome::Aborted => {}
            _ => panic!("expected Aborted after writer drop without finish"),
        }
    }

    #[tokio::test]
    async fn send_after_receiver_dropped_returns_stream_closed() {
        let (response, writer) = StreamingResponse::channel(200, None, 4);
        drop(response);
        let err = writer.send(b"x".to_vec()).await.unwrap_err();
        assert_eq!(err, StreamClosed);
    }

    #[tokio::test]
    async fn zero_capacity_is_rounded_up_to_one() {
        // capacity = 0 を渡しても panic せず、最低 1 は送信できることを確認する
        // （tokio::sync::mpsc::channel(0) は panic するため、コンストラクタ側で
        // 切り上げていることの固定）。
        let (mut response, writer) = StreamingResponse::channel(200, None, 0);
        writer.send(b"ok".to_vec()).await.unwrap();
        match response.recv().await {
            RecvOutcome::Chunk(data) => assert_eq!(data, b"ok"),
            _ => panic!("expected chunk"),
        }
    }

    #[test]
    fn stream_closed_display_and_error_trait() {
        fn assert_error<E: std::error::Error>() {}
        assert_error::<StreamClosed>();
        assert_eq!(
            StreamClosed.to_string(),
            "streaming response receiver has been closed"
        );
    }
}
