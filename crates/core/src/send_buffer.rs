//! 接続単位で再利用するレスポンス送信バッファ（イシュー #584、#595）。
//!
//! [`crate::server::handle_connection_with_permit`]（接続ループ）が 1
//! コネクションにつき [`SendBuffer`] を 1 つ保持し、応答ごとに
//! [`SendBuffer::serialize_response`] → `AsyncWriteExt::write_all`（戻り値の
//! スライスを書き出す）→ [`SendBuffer::shrink_if_oversized`] の順で使う契約。
//! `fandhe_backend_http::buffer::RecvBuffer`（受信側の接続単位再利用バッファ）
//! と対になる送信側の実装で、keep-alive 接続における応答ごとの `Vec` 新規
//! 確保（従来の `Response::serialize` 呼び出し）を接続の生存期間で 1 回に
//! 減らす。
//!
//! `crates/core` の接続ループ専用の内部実装で、`crates/http`（crates.io
//! 公開クレート `fandhe-backend-http`）の公開 API 面には出さない
//! （イシュー #595 レビュー指摘 P1「接続ループ専用の型を公開 API に追加
//! しない」対応。AGENTS.md「再利用・アセット化観点」の公開 API 面の汚染
//! 防止を参照）。`pub(crate)` に留め、`Vec<u8>` を包む薄いラッパーとして
//! crate 内でのみ使う。

use fandhe_backend_http::response::Response;

/// keep-alive 接続でレスポンス送信完了後に保持し続ける容量の上限。
///
/// `fandhe_backend_http::buffer::RecvBuffer` の `MAX_RETAINED_CAPACITY`
/// （64 KiB）と同一値・同一ポリシーを踏襲する（受信側・送信側で縮小基準を
/// 揃え、大 body 応答を送出した接続がその後も無条件に容量を保持し続けて
/// 多数接続時のメモリ滞留要因になるのを防ぐ、`.claude/rules/security.md`
/// リソース枯渇対策）。
const MAX_RETAINED_CAPACITY: usize = 64 * 1024;

/// 接続単位で再利用するレスポンス送信バッファ。
#[derive(Debug, Default)]
pub(crate) struct SendBuffer {
    buf: Vec<u8>,
}

impl SendBuffer {
    /// 空のバッファを作る（初回 [`Self::serialize_response`] 呼び出しまで
    /// heap alloc を行わない）。
    pub(crate) fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// `response` を `Response::serialize_into` で内部バッファへ直列化し、
    /// 送出可能なワイヤバイト列のスライスを返す。
    ///
    /// `Response::serialize_into` の契約どおり、呼び出し直後に内部バッファは
    /// `clear` されるため、前応答の残留バイトが混入することはない
    /// （レスポンス分割対策、`.claude/rules/security.md`）。
    pub(crate) fn serialize_response(&mut self, response: &Response, keep_alive: bool) -> &[u8] {
        response.serialize_into(keep_alive, &mut self.buf);
        &self.buf
    }

    /// 応答送信完了後に呼び、容量が [`MAX_RETAINED_CAPACITY`] を超えていれば
    /// 縮小する。
    pub(crate) fn shrink_if_oversized(&mut self) {
        if self.buf.capacity() <= MAX_RETAINED_CAPACITY {
            return;
        }
        self.buf.clear();
        self.buf.shrink_to(MAX_RETAINED_CAPACITY);
    }

    /// 内部バッファの現在の容量（バイト数）を返す。テスト補助 API。
    #[cfg(test)]
    pub(crate) fn capacity(&self) -> usize {
        self.buf.capacity()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn send_buffer_new_has_zero_capacity() {
        let buf = SendBuffer::new();
        assert_eq!(buf.capacity(), 0);
    }

    #[test]
    fn send_buffer_serialize_response_matches_response_serialize() {
        let mut send_buf = SendBuffer::new();
        let res = Response::new(200, b"payload".to_vec());
        let wire = send_buf.serialize_response(&res, true).to_vec();
        assert_eq!(wire, res.serialize(true));
    }

    #[test]
    fn send_buffer_reuses_capacity_across_two_responses_without_stale_bytes() {
        let mut send_buf = SendBuffer::new();
        let first = Response::new(200, b"first response".to_vec());
        let second = Response::empty(404);

        let first_wire = send_buf.serialize_response(&first, true).to_vec();
        assert_eq!(first_wire, first.serialize(true));
        let cap_after_first = send_buf.capacity();
        assert!(cap_after_first > 0);

        let second_wire = send_buf.serialize_response(&second, true).to_vec();
        // 2 回目の直列化が 1 回目のバイト列を混入させない（clear 契約が
        // SendBuffer 経由でも効いていることを end-to-end で確認）。
        assert_eq!(second_wire, second.serialize(true));
        // 2 回目は 1 回目より小さい応答のため、`Vec::clear` + `reserve` は
        // 再確保を起こさず容量を維持する（バッファが実際に再利用されている
        // ことの確認）。
        assert_eq!(send_buf.capacity(), cap_after_first);
    }

    #[test]
    fn send_buffer_shrink_if_oversized_shrinks_large_capacity() {
        let mut send_buf = SendBuffer::new();
        let big_body = vec![0xabu8; MAX_RETAINED_CAPACITY * 2];
        let res = Response::new(200, big_body);
        send_buf.serialize_response(&res, true);
        assert!(send_buf.capacity() > MAX_RETAINED_CAPACITY);

        send_buf.shrink_if_oversized();
        assert!(send_buf.capacity() <= MAX_RETAINED_CAPACITY);
    }

    #[test]
    fn send_buffer_shrink_if_oversized_keeps_capacity_at_or_below_threshold_untouched() {
        let mut send_buf = SendBuffer::new();
        let res = Response::new(200, b"small".to_vec());
        send_buf.serialize_response(&res, true);
        let cap_before = send_buf.capacity();
        assert!(cap_before <= MAX_RETAINED_CAPACITY);

        send_buf.shrink_if_oversized();
        assert_eq!(send_buf.capacity(), cap_before);
    }
}
