//! 接続単位で再利用する読み取りバッファ（TASK-1.3-3 / #68）。
//!
//! [`crate::connection::read_request`] の呼び出し元（コアの接続受理ループ、
//! TASK-1.4 / #13）は 1 コネクションにつき [`RecvBuffer`] を 1 つ保持し、
//! 繰り返し `read_request` へ渡す契約（#67 の `buf: Vec<u8>` 契約を引き継ぐ）。
//! [`RecvBuffer`] はこの契約を型として明示し、次の 2 点で memmove・ゼロ埋め
//! コストを削減する:
//!
//! 1. **遅延コンパクション**: 消費済みバイトはカーソル前進のみで扱い、
//!    パイプライン残余がある場合のみ次回読み取り直前に先頭詰めする
//!    （非パイプラインの典型ケースでは memmove がゼロになる）
//! 2. **ゼロ埋め回避**: 追加読み取りは `Vec::reserve` + `AsyncReadExt::read_buf`
//!    でスペア容量へ直接書き込み、`resize` によるゼロ埋めを行わない
//!
//! 容量はリクエスト処理完了時（クレート内部の縮小ポリシー）に
//! `MAX_RETAINED_CAPACITY` を超えないよう有界化し、大 body 処理後の
//! keep-alive 接続でのメモリ滞留を防ぐ
//! （.claude/rules/security.md のリソース枯渇対策）。

use tokio::io::{AsyncRead, AsyncReadExt};

/// 一括読み取りするチャンクサイズ。
///
/// 大きすぎると小さいリクエストでも無駄なメモリ確保が増え、小さすぎると
/// システムコール回数が増える。8 KiB は一般的な HTTP リクエストヘッドの
/// サイズ感（[`crate::request::MAX_HEADER_BYTES`] = 16 KiB）に対して妥当な
/// 折衷値として選んだ暫定値であり、定量チューニングは TASK-1.6（受け入れ
/// テスト）の計測結果で見直す。
pub(crate) const READ_CHUNK_BYTES: usize = 8 * 1024;

/// keep-alive 接続でリクエスト処理完了後に保持し続ける容量の上限。
///
/// [`crate::request::MAX_HEADER_BYTES`]（16 KiB）に、ヘッダ直後の body 先頭
/// チャンクが 1 回分パイプラインで届いても再確保が起きない余裕
/// （[`READ_CHUNK_BYTES`] × 数回分）を加えた 64 KiB を暫定値とする。大きい
/// body（最大 [`crate::body::MAX_BODY_BYTES`] = 1 MiB）を処理した接続が
/// keep-alive のまま生き続けても、この値を超えた容量は
/// [`RecvBuffer::shrink_if_oversized`] で解放し、多数接続時のメモリ滞留を
/// 接続単位で有界化する。定量的な最適値の検証は TASK-1.6 のスコープ。
pub(crate) const MAX_RETAINED_CAPACITY: usize = 64 * 1024;

/// 接続単位で再利用するソケット読み取りバッファ。
///
/// 内部に `Vec<u8>` と読み取り済み位置を示すカーソル `pos` を持つ。
/// 不変条件: 常に `pos <= buf.len()`。`unread()` が返すのは `buf[pos..]`
/// （まだ消費されていないバイト列、パイプライン残余を含む）。
#[derive(Debug, Default)]
pub struct RecvBuffer {
    buf: Vec<u8>,
    pos: usize,
}

impl RecvBuffer {
    /// 空のバッファを作る。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_http::buffer::RecvBuffer;
    ///
    /// let buf = RecvBuffer::new();
    /// assert!(buf.unread().is_empty());
    /// ```
    pub fn new() -> Self {
        Self {
            buf: Vec::new(),
            pos: 0,
        }
    }

    /// まだ消費されていないバイト列（パイプライン残余を含む）を返す。
    ///
    /// [`crate::request::parse_request_head`] の入力としてそのまま渡せる。
    pub fn unread(&self) -> &[u8] {
        &self.buf[self.pos..]
    }

    /// 内部バッファの現在の容量（バイト数）を返す。
    ///
    /// 呼び出し元（コアの接続受理ループ等）が keep-alive 接続での容量再利用・
    /// 縮小ポリシー（クレート内部の `shrink_if_oversized`）を観測・テストする
    /// ための補助 API。[`crate::body::MAX_BODY_BYTES`] 分のバッファ確保が
    /// 過剰でないことの監視や、統合テストでの容量再利用アサーションに使う。
    pub fn capacity(&self) -> usize {
        self.buf.capacity()
    }

    /// 先頭から `n` バイトを消費済みとしてカーソルを前進させる。
    ///
    /// `drain` による先頭詰めコピーを行わず、カーソル前進のみで消費を表現する
    /// （パイプライン残余がある非典型ケースでもここでは動かさない。実際の
    /// 詰め直しは次回の [`Self::reserve_for_read`] が必要に応じて行う）。
    ///
    /// # Panics
    ///
    /// `n` が現在の未読バイト数を超える場合は不変条件違反としてパニックする
    /// （呼び出し元のロジック誤りを示すバグであり、サイレントに握りつぶすと
    /// 後続の読み取り位置がずれて body 境界を誤る危険がある）。
    pub(crate) fn consume(&mut self, n: usize) {
        assert!(
            self.pos + n <= self.buf.len(),
            "RecvBuffer::consume: consumed more bytes than available"
        );
        self.pos += n;
    }

    /// 未読領域がちょうど `n` バイトのとき、コピーなしで取り出す。
    ///
    /// body 読み取りの典型ケース（ヘッド消費後、未読領域が body ちょうど）で
    /// [`Vec::to_vec`] によるコピーを避けるための最適化。`unread().len() != n`
    /// の場合（パイプライン残余がある／まだ body 全体が届いていない）は
    /// `None` を返し、呼び出し元はコピーで対応する。
    ///
    /// 取り出し後のバッファは空になる（次回 [`Self::reserve_for_read`] が
    /// スペア容量として使う）。
    pub(crate) fn take_exact(&mut self, n: usize) -> Option<Vec<u8>> {
        if self.pos == 0 && self.buf.len() == n {
            self.pos = 0;
            Some(std::mem::take(&mut self.buf))
        } else {
            None
        }
    }

    /// 未読領域を保ったまま、追加読み取りのためのスペア容量を確保する。
    ///
    /// 未読領域が空なら容量を維持したまま `clear()`（O(1)）。未読領域が
    /// 残っている場合（パイプライン残余）のみ `copy_within` で先頭へ詰め、
    /// 以降のヘッド／body パースが常に `buf[0..]` を起点にできるようにする。
    /// 詰め直し後、[`READ_CHUNK_BYTES`] 分の空き容量を `reserve` する
    /// （`resize` によるゼロ埋めは行わない）。
    fn reserve_for_read(&mut self) {
        if self.pos > 0 {
            let unread_len = self.buf.len() - self.pos;
            if unread_len > 0 {
                self.buf.copy_within(self.pos.., 0);
            }
            self.buf.truncate(unread_len);
            self.pos = 0;
        }
        self.buf.reserve(READ_CHUNK_BYTES);
    }

    /// リクエスト処理完了時、容量が [`MAX_RETAINED_CAPACITY`] を超えていれば
    /// 縮小する。
    ///
    /// keep-alive 接続は同じ [`RecvBuffer`] を繰り返し使うため、大 body
    /// （最大 [`crate::body::MAX_BODY_BYTES`]）を処理した直後の容量を無条件に
    /// 保持し続けると多数接続時のメモリ滞留要因になる
    /// （.claude/rules/security.md リソース枯渇対策）。消費済みの先頭バイト列は
    /// 縮小前にカーソル前詰め（`copy_within`）で捨て、`Vec::len()` に残った
    /// 消費済み分が縮小目標を無駄に押し上げないようにする。未読領域
    /// （パイプライン残余）は縮小後も保持されたまま失われない。
    pub(crate) fn shrink_if_oversized(&mut self) {
        if self.buf.capacity() <= MAX_RETAINED_CAPACITY {
            return;
        }
        if self.pos > 0 {
            let unread_len = self.buf.len() - self.pos;
            if unread_len > 0 {
                self.buf.copy_within(self.pos.., 0);
            }
            self.buf.truncate(unread_len);
            self.pos = 0;
        }
        self.buf
            .shrink_to(MAX_RETAINED_CAPACITY.max(self.buf.len()));
    }

    /// `reader` から最大 [`READ_CHUNK_BYTES`] バイトを読み取り、未読領域の
    /// 末尾に追記する。
    ///
    /// 戻り値は読み取ったバイト数（`0` は EOF を意味する）。`AsyncReadExt::
    /// read_buf` はスペア容量（`Vec` の未初期化領域を安全に扱う `BufMut`
    /// 実装）へ直接書き込むため、`resize` によるゼロ埋めが発生しない。
    /// `unsafe` は使わない。
    pub(crate) async fn read_chunk<R: AsyncRead + Unpin>(
        &mut self,
        reader: &mut R,
    ) -> std::io::Result<usize> {
        self.reserve_for_read();
        reader.read_buf(&mut self.buf).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_buffer_is_empty() {
        let buf = RecvBuffer::new();
        assert!(buf.unread().is_empty());
    }

    #[test]
    fn consume_advances_cursor_without_moving_bytes() {
        let mut buf = RecvBuffer::new();
        let head = b"GET /a HTTP/1.1\r\n\r\n";
        buf.buf = [head.as_slice(), b"rest"].concat();
        buf.consume(head.len());
        assert_eq!(buf.unread(), b"rest");
    }

    #[test]
    #[should_panic(expected = "consumed more bytes than available")]
    fn consume_beyond_available_panics() {
        let mut buf = RecvBuffer::new();
        buf.buf = b"abc".to_vec();
        buf.consume(10);
    }

    #[test]
    fn take_exact_avoids_copy_when_unread_matches_exactly() {
        let mut buf = RecvBuffer::new();
        buf.buf = b"abcd".to_vec();
        let taken = buf.take_exact(4).expect("should take exact match");
        assert_eq!(taken, b"abcd");
        assert!(buf.unread().is_empty());
    }

    #[test]
    fn take_exact_returns_none_when_pipelined_remainder_present() {
        let mut buf = RecvBuffer::new();
        buf.buf = b"abcdEXTRA".to_vec();
        assert_eq!(buf.take_exact(4), None);
    }

    #[test]
    fn take_exact_returns_none_when_cursor_advanced() {
        let mut buf = RecvBuffer::new();
        buf.buf = b"HEADabcd".to_vec();
        buf.consume(4);
        // 未読領域は "abcd" で長さは一致するが pos != 0 のため、
        // 先頭詰めが必要でコピー回避の前提（pos == 0）を満たさない。
        assert_eq!(buf.take_exact(4), None);
    }

    #[test]
    fn reserve_for_read_compacts_pipelined_remainder_to_front() {
        let mut buf = RecvBuffer::new();
        buf.buf = b"HEADrest".to_vec();
        buf.consume(4);
        buf.reserve_for_read();
        assert_eq!(buf.unread(), b"rest");
        assert_eq!(buf.pos, 0);
    }

    #[test]
    fn reserve_for_read_is_noop_move_when_fully_consumed() {
        let mut buf = RecvBuffer::new();
        buf.buf = b"HEAD".to_vec();
        buf.consume(4);
        buf.reserve_for_read();
        assert!(buf.unread().is_empty());
        assert_eq!(buf.pos, 0);
    }

    #[test]
    fn shrink_if_oversized_shrinks_large_capacity() {
        let mut buf = RecvBuffer::new();
        buf.buf = Vec::with_capacity(MAX_RETAINED_CAPACITY * 4);
        buf.buf.extend_from_slice(b"remainder");
        buf.shrink_if_oversized();
        assert!(buf.buf.capacity() <= MAX_RETAINED_CAPACITY.max(buf.buf.len()));
        assert_eq!(buf.unread(), b"remainder");
    }

    #[test]
    fn shrink_if_oversized_keeps_capacity_at_or_below_threshold_untouched() {
        let mut buf = RecvBuffer::new();
        buf.buf = Vec::with_capacity(1024);
        buf.buf.extend_from_slice(b"data");
        let cap_before = buf.buf.capacity();
        buf.shrink_if_oversized();
        assert_eq!(buf.buf.capacity(), cap_before);
    }

    #[tokio::test]
    async fn read_chunk_reads_into_spare_capacity() {
        let mut socket: &[u8] = b"hello world";
        let mut buf = RecvBuffer::new();
        let n = buf.read_chunk(&mut socket).await.unwrap();
        assert_eq!(n, 11);
        assert_eq!(buf.unread(), b"hello world");
    }

    #[tokio::test]
    async fn read_chunk_appends_after_existing_unread_data() {
        let mut socket: &[u8] = b"world";
        let mut buf = RecvBuffer::new();
        buf.buf = b"hello ".to_vec();
        let n = buf.read_chunk(&mut socket).await.unwrap();
        assert_eq!(n, 5);
        assert_eq!(buf.unread(), b"hello world");
    }

    #[tokio::test]
    async fn read_chunk_returns_zero_on_eof() {
        let mut socket: &[u8] = b"";
        let mut buf = RecvBuffer::new();
        let n = buf.read_chunk(&mut socket).await.unwrap();
        assert_eq!(n, 0);
    }
}
