//! chunked transfer-coding のデコーダ（RFC 9112 §7.1、sans-IO、イシュー #181）。
//!
//! [`crate::body::body_length`] が `BodyLength::Chunked` と判定したリクエストの
//! body 部分を、ソケット I/O を持つ `read_body_chunked`
//! （呼び出し元）がインクリメンタルに読み進めるための純粋な状態機械を提供する。
//! [`crate::request::parse_request_head`] と同じ「入力不足は `Incomplete` を返し、
//! 呼び出し元が追い読みして同じデコーダへ再入力する」パターンに従う。
//!
//! # DoS 耐性（.claude/rules/security.md リソース枯渇対策）
//!
//! 前段プロキシとの解釈差異（リクエストスマグリング）を避けるため、構文は
//! CRLF 厳格（bare LF は拒否）・hex 桁は ASCII `0`-`9`/`a`-`f`/`A`-`F` のみを
//! 許容する。加えて次の上限をすべてバッファ確保前に検査し、fail-closed で
//! 拒否する:
//!
//! - 復号後総量: [`crate::body::MAX_BODY_BYTES`] を再利用（サイズ宣言の加算
//!   時点で判定するため、超過分を実際にバッファへ確保しない）
//! - チャンク総数: [`MAX_CHUNK_COUNT`]（1 バイトチャンク連打による CPU 消費・
//!   断片化 DoS 対策）
//! - chunk-size 行長（拡張含む）: [`MAX_CHUNK_LINE_BYTES`]（行の無限バッファ
//!   リング防止。chunk 拡張は解釈せず読み捨てるが行長は有界化する）
//! - trailer: 非空 trailer フィールドは受理しない（安全側デフォルト。空
//!   trailer、すなわち最終チャンク直後の即 CRLF のみを受理する）

use crate::body::MAX_BODY_BYTES;

/// chunked transfer-coding のエンコーダ（sans-IO、イシュー #319）。
///
/// [`ChunkedDecoder`] と対になる純関数群。新規の状態を持たず、呼び出し元
/// （`crates/core/src/server.rs` の書き出しループ）が任意のタイミングで
/// 呼び出して `out` へ追記できる。ソケット I/O・バッファ確保戦略は一切
/// 関知しない sans-IO 設計を [`ChunkedDecoder`] と共通させる。
///
/// `data` に 1 チャンク分のバイト列を書き込む。
///
/// `<hex-size>\r\n<data>\r\n` の形式で `out` へ追記する。
///
/// # 空チャンクの扱い（レスポンス誤終端の構造的防止）
///
/// RFC 9112 §7.1 上、chunk-size `0` は終端（last-chunk）専用の予約値であり、
/// 通常チャンクとして送出してはならない。`data` が空の場合に素直に
/// `0\r\n\r\n` を出力すると、ストリーミング途中で意図せず応答を終端させて
/// しまう（呼び出し元がまだ後続チャンクを送るつもりでも、受信側は
/// 完全な応答を受け取ったと誤認する）。本関数は **空データのときは何も
/// 出力しない**契約とし、終端は必ず [`encode_terminator`] を明示的に
/// 呼ぶ経路のみに限定する（`.claude/rules/security.md` のレスポンス完全性・
/// フェイルクローズ方針）。
///
/// ```
/// use fandhe_backend_http::chunked::encode_chunk;
///
/// let mut out = Vec::new();
/// encode_chunk(b"Wiki", &mut out);
/// assert_eq!(out, b"4\r\nWiki\r\n");
///
/// // 空データは無出力（誤終端防止）。
/// let mut out = Vec::new();
/// encode_chunk(b"", &mut out);
/// assert!(out.is_empty());
/// ```
pub fn encode_chunk(data: &[u8], out: &mut Vec<u8>) {
    if data.is_empty() {
        return;
    }
    out.extend_from_slice(format!("{:x}", data.len()).as_bytes());
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(data);
    out.extend_from_slice(b"\r\n");
}

/// chunked body の終端（last-chunk + 空 trailer + 空行）を `out` へ追記する。
///
/// `0\r\n\r\n` を出力する。[`ChunkedDecoder`] が「空 trailer のみ受理」する
/// 方針（モジュール冒頭 doc）と対称的に、trailer フィールドは一切出力
/// しない（送出側は trailer を持たない）。
///
/// ```
/// use fandhe_backend_http::chunked::encode_terminator;
///
/// let mut out = Vec::new();
/// encode_terminator(&mut out);
/// assert_eq!(out, b"0\r\n\r\n");
/// ```
pub fn encode_terminator(out: &mut Vec<u8>) {
    out.extend_from_slice(b"0\r\n\r\n");
}

/// 1 リクエストで許容するチャンク総数の上限。
///
/// 1 バイトチャンクを大量に送りつける CPU・メモリ断片化 DoS を防ぐ
/// （.claude/rules/security.md）。
pub const MAX_CHUNK_COUNT: u64 = 16_384;

/// chunk-size 行（chunk 拡張込み）として許容する最大バイト数。
///
/// 行末（CRLF）に到達しないまま累積バイト数がこの値を超えた場合は
/// [`ChunkedError::ChunkLineTooLong`] を返し、無限バッファ成長を防ぐ。
pub const MAX_CHUNK_LINE_BYTES: usize = 256;

/// [`ChunkedDecoder::decode`] が返しうるエラー。
#[derive(Debug, PartialEq, Eq)]
pub enum ChunkedError {
    /// chunk-size 行が hex 数字のみで構成される非空文字列でない
    /// （chunk 拡張部分を除く）、または `u64` の範囲を超える。
    InvalidChunkSize,
    /// chunk-size 行（拡張含む）が [`MAX_CHUNK_LINE_BYTES`] を超えた。
    ChunkLineTooLong,
    /// チャンク総数が [`MAX_CHUNK_COUNT`] を超えた。
    TooManyChunks,
    /// 復号後の body 総量が [`crate::body::MAX_BODY_BYTES`] を超えた
    /// （個別チャンクサイズがこの上限を単独で超える場合も含む）。
    BodyTooLarge,
    /// 非空の trailer フィールドを受理しない（安全側デフォルト）。
    TrailerUnsupported,
    /// 行終端が `\r\n` でない（bare LF 等）。
    InvalidLineTerminator,
}

impl std::fmt::Display for ChunkedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            ChunkedError::InvalidChunkSize => "invalid chunk size",
            ChunkedError::ChunkLineTooLong => "chunk size line exceeds MAX_CHUNK_LINE_BYTES",
            ChunkedError::TooManyChunks => "chunk count exceeds MAX_CHUNK_COUNT",
            ChunkedError::BodyTooLarge => "decoded chunked body exceeds MAX_BODY_BYTES",
            ChunkedError::TrailerUnsupported => "non-empty chunked trailer is not supported",
            ChunkedError::InvalidLineTerminator => "chunked line terminator must be CRLF",
        };
        f.write_str(msg)
    }
}

impl std::error::Error for ChunkedError {}

/// [`ChunkedDecoder::decode`] の呼び出し 1 回分の結果。
#[derive(Debug, PartialEq, Eq)]
pub enum DecodeOutcome {
    /// 終端（`0` チャンクサイズ + 空 trailer + CRLF）まで到達した。
    Complete {
        /// 今回の `decode` 呼び出しの入力（`input` 引数）から消費したバイト数。
        /// 呼び出し元はこの値だけ読み取りバッファを前進させる。
        consumed: usize,
    },
    /// 入力が不足しており、追い読みが必要。
    Incomplete {
        /// 今回の `decode` 呼び出しの入力から消費（デコード完了）した分の
        /// バイト数。呼び出し元は追い読みしたバイト列を含む次の未読領域
        /// 全体を、同じデコーダへ再度渡す契約（内部状態は保持される）。
        consumed: usize,
    },
}

/// chunk-size 行 → chunk-data → 次チャンク（または trailer）と遷移する内部状態。
#[derive(Debug)]
enum State {
    /// chunk-size 行（`<hex>[;ext...]CRLF`）を読み取り中。
    ChunkSizeLine,
    /// chunk-data を `remaining` バイト読み取り中。
    ChunkData { remaining: u64 },
    /// chunk-data 直後の CRLF を読み取り中。
    ChunkDataCrlf,
    /// 最終チャンク（size 0）直後の trailer 行を読み取り中。
    ///
    /// 本実装は非空 trailer を受理しないため、この状態では「空行（即
    /// CRLF）」以外はすべて [`ChunkedError::TrailerUnsupported`] とする。
    TrailerLine,
    /// 終端まで到達済み。以降の `decode` 呼び出しは常に `Complete` を返す。
    Done,
}

/// chunked transfer-coding のインクリメンタルデコーダ（sans-IO）。
///
/// `read_body_chunked` から、ソケットから読み取った
/// バイト列を繰り返し [`Self::decode`] へ渡される契約。`decode` は
/// [`DecodeOutcome::Incomplete`] を返した場合、呼び出し元が追い読みして
/// 再入力するまで内部状態（chunk-size 行の途中バイト列・残りチャンク長・
/// 総復号量・チャンク数）を保持する。
///
/// # Examples
///
/// ```
/// use fandhe_backend_http::chunked::{ChunkedDecoder, DecodeOutcome};
///
/// let mut decoder = ChunkedDecoder::new();
/// let mut out = Vec::new();
/// let input = b"4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n";
/// let outcome = decoder.decode(input, &mut out).unwrap();
/// assert_eq!(outcome, DecodeOutcome::Complete { consumed: input.len() });
/// assert_eq!(out, b"Wikipedia");
/// ```
#[derive(Debug)]
pub struct ChunkedDecoder {
    state: State,
    /// 行ベースの状態（`ChunkSizeLine` / `ChunkDataCrlf` / `TrailerLine`）で
    /// CRLF を跨いで蓄積する行バッファ。CRLF 到達ごとに空になる。
    line_buf: Vec<u8>,
    /// これまでに復号した body の総バイト数（[`Self::max_body_bytes`] 有界化用）。
    total_decoded: u64,
    /// これまでに読んだチャンク数（[`MAX_CHUNK_COUNT`] 有界化用）。
    chunk_count: u64,
    /// 復号後総量の上限。既定は [`MAX_BODY_BYTES`]、
    /// [`Self::with_max_body_bytes`] で上書き可（`Server::max_body_bytes`、
    /// イシュー #311）。
    max_body_bytes: u64,
}

impl Default for ChunkedDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl ChunkedDecoder {
    /// 新規デコーダを作る（`ChunkSizeLine` 状態から開始、上限は既定の
    /// [`MAX_BODY_BYTES`]）。
    pub fn new() -> Self {
        Self::with_max_body_bytes(MAX_BODY_BYTES)
    }

    /// 復号後総量の上限を `max_body_bytes` にした新規デコーダを作る。
    ///
    /// `Server::max_body_bytes`（イシュー #311）で上限を上書きした場合に、
    /// `read_body_chunked`（`crates/http/src/connection.rs`）がこのコンストラクタ
    /// 経由でデコーダを生成する。`max_body_bytes == 0` は「chunk-data を一切
    /// 含まないチャンク列」のみを受理する（`0\r\n\r\n` は受理、それ以外の
    /// chunk-size 宣言は即座に [`ChunkedError::BodyTooLarge`]）。
    ///
    /// # Examples
    ///
    /// ```
    /// use fandhe_backend_http::chunked::{ChunkedDecoder, DecodeOutcome};
    ///
    /// let mut decoder = ChunkedDecoder::with_max_body_bytes(4);
    /// let mut out = Vec::new();
    /// let outcome = decoder.decode(b"4\r\nWiki\r\n0\r\n\r\n", &mut out).unwrap();
    /// assert_eq!(outcome, DecodeOutcome::Complete { consumed: 14 });
    /// assert_eq!(out, b"Wiki");
    /// ```
    pub fn with_max_body_bytes(max_body_bytes: u64) -> Self {
        Self {
            state: State::ChunkSizeLine,
            line_buf: Vec::new(),
            total_decoded: 0,
            chunk_count: 0,
            max_body_bytes,
        }
    }

    /// `input` の先頭から可能な限りデコードし、復号済みバイト列を `out` へ
    /// 追記する。
    ///
    /// 呼び出し元は [`DecodeOutcome::Incomplete`] を受け取った場合、`input`
    /// から `consumed` 分を読み取り済みとしてバッファを前進させたうえで
    /// 追い読みし、拡張された未読領域全体を次回の `decode` 呼び出しの
    /// `input` として渡す（`read_body_chunked` の契約）。
    pub fn decode(
        &mut self,
        input: &[u8],
        out: &mut Vec<u8>,
    ) -> Result<DecodeOutcome, ChunkedError> {
        let mut pos = 0usize;
        loop {
            match self.state {
                State::Done => return Ok(DecodeOutcome::Complete { consumed: pos }),
                State::ChunkSizeLine => match Self::take_line(&mut self.line_buf, input, &mut pos)?
                {
                    None => return Ok(DecodeOutcome::Incomplete { consumed: pos }),
                    Some(line) => {
                        let size = parse_chunk_size(&line)?;
                        self.chunk_count += 1;
                        if self.chunk_count > MAX_CHUNK_COUNT {
                            return Err(ChunkedError::TooManyChunks);
                        }
                        if size == 0 {
                            self.state = State::TrailerLine;
                        } else {
                            let new_total = self
                                .total_decoded
                                .checked_add(size)
                                .ok_or(ChunkedError::BodyTooLarge)?;
                            if new_total > self.max_body_bytes {
                                return Err(ChunkedError::BodyTooLarge);
                            }
                            self.state = State::ChunkData { remaining: size };
                        }
                    }
                },
                State::ChunkData { remaining } => {
                    let available = input.len() - pos;
                    if available == 0 {
                        return Ok(DecodeOutcome::Incomplete { consumed: pos });
                    }
                    let take = std::cmp::min(available as u64, remaining) as usize;
                    out.extend_from_slice(&input[pos..pos + take]);
                    self.total_decoded += take as u64;
                    pos += take;
                    let remaining_left = remaining - take as u64;
                    if remaining_left == 0 {
                        self.state = State::ChunkDataCrlf;
                    } else {
                        self.state = State::ChunkData {
                            remaining: remaining_left,
                        };
                        return Ok(DecodeOutcome::Incomplete { consumed: pos });
                    }
                }
                State::ChunkDataCrlf => match Self::take_line(&mut self.line_buf, input, &mut pos)?
                {
                    None => return Ok(DecodeOutcome::Incomplete { consumed: pos }),
                    Some(line) => {
                        if !line.is_empty() {
                            return Err(ChunkedError::InvalidLineTerminator);
                        }
                        self.state = State::ChunkSizeLine;
                    }
                },
                State::TrailerLine => match Self::take_line(&mut self.line_buf, input, &mut pos)? {
                    None => return Ok(DecodeOutcome::Incomplete { consumed: pos }),
                    Some(line) => {
                        if !line.is_empty() {
                            return Err(ChunkedError::TrailerUnsupported);
                        }
                        self.state = State::Done;
                        return Ok(DecodeOutcome::Complete { consumed: pos });
                    }
                },
            }
        }
    }

    /// `line_buf` に蓄積しつつ `input[*pos..]` を CRLF まで走査し、見つかれば
    /// `\r\n` を除いた行（CR も含めない）を返して `line_buf` を空にする。
    ///
    /// CRLF に到達する前に `input` が尽きた場合は `None` を返し、蓄積済みの
    /// バイト列は `line_buf` に残したまま次回呼び出しへ引き継ぐ（複数回の
    /// `decode` 呼び出しを跨いだ行分割に対応するため）。bare LF（直前が `\r`
    /// でない `\n`）は [`ChunkedError::InvalidLineTerminator`] として拒否する。
    fn take_line(
        line_buf: &mut Vec<u8>,
        input: &[u8],
        pos: &mut usize,
    ) -> Result<Option<Vec<u8>>, ChunkedError> {
        while *pos < input.len() {
            let byte = input[*pos];
            *pos += 1;
            if byte == b'\n' {
                if line_buf.last() != Some(&b'\r') {
                    return Err(ChunkedError::InvalidLineTerminator);
                }
                line_buf.pop();
                return Ok(Some(std::mem::take(line_buf)));
            }
            line_buf.push(byte);
            if line_buf.len() > MAX_CHUNK_LINE_BYTES {
                return Err(ChunkedError::ChunkLineTooLong);
            }
        }
        Ok(None)
    }
}

/// chunk-size 行から chunk 拡張（`;` 以降）を除いた hex 部分を `u64` として解析する。
///
/// hex 部分は ASCII hex digit のみで構成される非空文字列であることを要求し、
/// それ以外（空文字列・非 hex 文字・`u64` オーバーフロー）はすべて
/// [`ChunkedError::InvalidChunkSize`] として拒否する。chunk 拡張の内容は
/// 解釈せず読み捨てる（読み捨て自体の行長有界化は [`ChunkedDecoder::take_line`]
/// が担う）。
fn parse_chunk_size(line: &[u8]) -> Result<u64, ChunkedError> {
    let size_part = match line.iter().position(|&b| b == b';') {
        Some(idx) => &line[..idx],
        None => line,
    };
    if size_part.is_empty() || !size_part.iter().all(u8::is_ascii_hexdigit) {
        return Err(ChunkedError::InvalidChunkSize);
    }
    let s = std::str::from_utf8(size_part).map_err(|_| ChunkedError::InvalidChunkSize)?;
    u64::from_str_radix(s, 16).map_err(|_| ChunkedError::InvalidChunkSize)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode_all(input: &[u8]) -> Result<(DecodeOutcome, Vec<u8>), ChunkedError> {
        let mut decoder = ChunkedDecoder::new();
        let mut out = Vec::new();
        let outcome = decoder.decode(input, &mut out)?;
        Ok((outcome, out))
    }

    #[test]
    fn single_chunk_is_decoded() {
        let (outcome, out) = decode_all(b"4\r\nWiki\r\n0\r\n\r\n").unwrap();
        assert_eq!(
            outcome,
            DecodeOutcome::Complete {
                consumed: b"4\r\nWiki\r\n0\r\n\r\n".len()
            }
        );
        assert_eq!(out, b"Wiki");
    }

    #[test]
    fn multiple_chunks_are_concatenated() {
        let (outcome, out) = decode_all(b"4\r\nWiki\r\n5\r\npedia\r\n0\r\n\r\n").unwrap();
        assert!(matches!(outcome, DecodeOutcome::Complete { .. }));
        assert_eq!(out, b"Wikipedia");
    }

    #[test]
    fn empty_body_is_decoded() {
        let (outcome, out) = decode_all(b"0\r\n\r\n").unwrap();
        assert!(matches!(outcome, DecodeOutcome::Complete { .. }));
        assert!(out.is_empty());
    }

    #[test]
    fn chunk_extension_is_ignored() {
        let (outcome, out) = decode_all(b"4;ext=1\r\nWiki\r\n0\r\n\r\n").unwrap();
        assert!(matches!(outcome, DecodeOutcome::Complete { .. }));
        assert_eq!(out, b"Wiki");
    }

    #[test]
    fn uppercase_hex_size_is_accepted() {
        let (outcome, out) = decode_all(b"A\r\n0123456789\r\n0\r\n\r\n").unwrap();
        assert!(matches!(outcome, DecodeOutcome::Complete { .. }));
        assert_eq!(out, b"0123456789");
    }

    #[test]
    fn incomplete_input_returns_incomplete_and_can_be_resumed() {
        let mut decoder = ChunkedDecoder::new();
        let mut out = Vec::new();

        let outcome = decoder.decode(b"4\r\nWi", &mut out).unwrap();
        assert!(matches!(outcome, DecodeOutcome::Incomplete { .. }));
        assert_eq!(out, b"Wi");

        // 呼び出し元は「消費済みの続き」ではなく、追い読みしたバイト列を
        // 含む新しい未読領域全体を渡す契約（RecvBuffer の consume 契約に
        // 合わせる）。ここでは残りの body + 終端を渡して再開できることを
        // 固定する。
        let outcome = decoder.decode(b"ki\r\n0\r\n\r\n", &mut out).unwrap();
        assert!(matches!(outcome, DecodeOutcome::Complete { .. }));
        assert_eq!(out, b"Wiki");
    }

    #[test]
    fn split_across_many_single_byte_calls() {
        let mut decoder = ChunkedDecoder::new();
        let mut out = Vec::new();
        let input = b"4\r\nWiki\r\n0\r\n\r\n";
        let mut complete = false;
        for &byte in input {
            let outcome = decoder.decode(&[byte], &mut out).unwrap();
            if matches!(outcome, DecodeOutcome::Complete { .. }) {
                complete = true;
            }
        }
        assert!(complete);
        assert_eq!(out, b"Wiki");
    }

    #[test]
    fn invalid_hex_chunk_size_is_rejected() {
        let err = decode_all(b"XYZ\r\n").unwrap_err();
        assert_eq!(err, ChunkedError::InvalidChunkSize);
    }

    #[test]
    fn empty_chunk_size_is_rejected() {
        let err = decode_all(b"\r\nabcd\r\n0\r\n\r\n").unwrap_err();
        assert_eq!(err, ChunkedError::InvalidChunkSize);
    }

    #[test]
    fn overflowing_chunk_size_is_rejected() {
        let err = decode_all(b"FFFFFFFFFFFFFFFFF\r\n").unwrap_err();
        assert_eq!(err, ChunkedError::InvalidChunkSize);
    }

    #[test]
    fn bare_lf_in_chunk_size_line_is_rejected() {
        let err = decode_all(b"4\nWiki\r\n0\r\n\r\n").unwrap_err();
        assert_eq!(err, ChunkedError::InvalidLineTerminator);
    }

    #[test]
    fn bare_lf_after_chunk_data_is_rejected() {
        let err = decode_all(b"4\r\nWiki\n0\r\n\r\n").unwrap_err();
        assert_eq!(err, ChunkedError::InvalidLineTerminator);
    }

    #[test]
    fn missing_chunk_data_crlf_is_rejected() {
        let err = decode_all(b"4\r\nWikiXX0\r\n\r\n").unwrap_err();
        assert_eq!(err, ChunkedError::InvalidLineTerminator);
    }

    #[test]
    fn non_empty_trailer_is_rejected() {
        let err = decode_all(b"0\r\nX-Trailer: value\r\n\r\n").unwrap_err();
        assert_eq!(err, ChunkedError::TrailerUnsupported);
    }

    #[test]
    fn chunk_size_line_too_long_is_rejected() {
        let mut input = vec![b'1'; MAX_CHUNK_LINE_BYTES + 1];
        input.extend_from_slice(b"\r\n");
        let err = decode_all(&input).unwrap_err();
        assert_eq!(err, ChunkedError::ChunkLineTooLong);
    }

    #[test]
    fn single_chunk_exceeding_max_body_bytes_is_rejected() {
        let value = format!("{:X}\r\n", MAX_BODY_BYTES + 1);
        let err = decode_all(value.as_bytes()).unwrap_err();
        assert_eq!(err, ChunkedError::BodyTooLarge);
    }

    #[test]
    fn chunk_at_exact_max_body_bytes_is_accepted() {
        // サイズ宣言のみを検証し、実際の巨大データは送らない（テスト時間・
        // メモリの節約。デコーダは chunk-size 検証時点で判定するため、
        // 宣言だけで上限一致が受理されることを固定できる）。
        let mut decoder = ChunkedDecoder::new();
        let mut out = Vec::new();
        let header = format!("{MAX_BODY_BYTES:X}\r\n");
        let outcome = decoder.decode(header.as_bytes(), &mut out).unwrap();
        assert!(matches!(outcome, DecodeOutcome::Incomplete { .. }));
    }

    #[test]
    fn total_decoded_across_chunks_exceeding_max_body_bytes_is_rejected() {
        let half = MAX_BODY_BYTES / 2;
        let input = format!("{half:X}\r\n");
        let mut decoder = ChunkedDecoder::new();
        let mut out = Vec::new();
        // 1 チャンク目のサイズ宣言のみ流し、chunk-data 本体は送らない
        // （メモリ節約）。2 チャンク目で合計が上限を超えることを検証する。
        let outcome = decoder.decode(input.as_bytes(), &mut out).unwrap();
        assert!(matches!(outcome, DecodeOutcome::Incomplete { .. }));

        // 1 チャンク目分のダミーデータと CRLF、2 チャンク目のサイズ宣言を渡す。
        let mut rest = vec![0u8; half as usize];
        rest.extend_from_slice(b"\r\n");
        rest.extend_from_slice(format!("{:X}\r\n", MAX_BODY_BYTES - half + 2).as_bytes());
        let err = decoder.decode(&rest, &mut out).unwrap_err();
        assert_eq!(err, ChunkedError::BodyTooLarge);
    }

    #[test]
    fn too_many_chunks_is_rejected() {
        let mut decoder = ChunkedDecoder::new();
        let mut out = Vec::new();
        let one_byte_chunk = b"1\r\nA\r\n";
        let mut result = Ok(DecodeOutcome::Incomplete { consumed: 0 });
        for _ in 0..=MAX_CHUNK_COUNT {
            result = decoder.decode(one_byte_chunk, &mut out);
            if result.is_err() {
                break;
            }
        }
        assert_eq!(result.unwrap_err(), ChunkedError::TooManyChunks);
    }

    #[test]
    fn decode_after_done_is_idempotent_complete() {
        let mut decoder = ChunkedDecoder::new();
        let mut out = Vec::new();
        decoder.decode(b"0\r\n\r\n", &mut out).unwrap();
        let outcome = decoder.decode(b"garbage", &mut out).unwrap();
        assert_eq!(outcome, DecodeOutcome::Complete { consumed: 0 });
    }

    #[test]
    fn chunked_error_display_messages_are_stable() {
        // Display 文言の固定（PoC-9 教訓、body.rs body_error_display_messages_are_stable
        // と同一方針）。上位（RequestError::Chunked 経由）でそのまま連結される。
        assert_eq!(
            ChunkedError::InvalidChunkSize.to_string(),
            "invalid chunk size"
        );
        assert_eq!(
            ChunkedError::ChunkLineTooLong.to_string(),
            "chunk size line exceeds MAX_CHUNK_LINE_BYTES"
        );
        assert_eq!(
            ChunkedError::TooManyChunks.to_string(),
            "chunk count exceeds MAX_CHUNK_COUNT"
        );
        assert_eq!(
            ChunkedError::BodyTooLarge.to_string(),
            "decoded chunked body exceeds MAX_BODY_BYTES"
        );
        assert_eq!(
            ChunkedError::TrailerUnsupported.to_string(),
            "non-empty chunked trailer is not supported"
        );
        assert_eq!(
            ChunkedError::InvalidLineTerminator.to_string(),
            "chunked line terminator must be CRLF"
        );
    }

    #[test]
    fn chunked_error_implements_std_error() {
        fn assert_error<E: std::error::Error>() {}
        assert_error::<ChunkedError>();
    }

    #[test]
    fn with_max_body_bytes_custom_limit_accepts_at_boundary() {
        let mut decoder = ChunkedDecoder::with_max_body_bytes(4);
        let mut out = Vec::new();
        let outcome = decoder.decode(b"4\r\nWiki\r\n0\r\n\r\n", &mut out).unwrap();
        assert!(matches!(outcome, DecodeOutcome::Complete { .. }));
        assert_eq!(out, b"Wiki");
    }

    #[test]
    fn with_max_body_bytes_custom_limit_rejects_over_boundary() {
        let mut decoder = ChunkedDecoder::with_max_body_bytes(4);
        let mut out = Vec::new();
        let err = decoder
            .decode(b"5\r\nWikip\r\n0\r\n\r\n", &mut out)
            .unwrap_err();
        assert_eq!(err, ChunkedError::BodyTooLarge);
    }

    #[test]
    fn with_max_body_bytes_zero_rejects_any_chunk_data() {
        let mut decoder = ChunkedDecoder::with_max_body_bytes(0);
        let mut out = Vec::new();
        let err = decoder
            .decode(b"1\r\nA\r\n0\r\n\r\n", &mut out)
            .unwrap_err();
        assert_eq!(err, ChunkedError::BodyTooLarge);
    }

    #[test]
    fn with_max_body_bytes_zero_accepts_empty_body() {
        let mut decoder = ChunkedDecoder::with_max_body_bytes(0);
        let mut out = Vec::new();
        let outcome = decoder.decode(b"0\r\n\r\n", &mut out).unwrap();
        assert!(matches!(outcome, DecodeOutcome::Complete { .. }));
        assert!(out.is_empty());
    }

    #[test]
    fn new_uses_default_max_body_bytes() {
        // ChunkedDecoder::new() が MAX_BODY_BYTES を既定値として使う wrapper
        // であることの固定（body.rs body_length_with_limit_matches_default_wrapper
        // と同一方針）。
        let value = format!("{:X}\r\n", MAX_BODY_BYTES + 1);
        let err = decode_all(value.as_bytes()).unwrap_err();
        assert_eq!(err, ChunkedError::BodyTooLarge);
    }

    // --- encode_chunk / encode_terminator（イシュー #319） ---

    #[test]
    fn encode_chunk_writes_hex_size_and_data() {
        let mut out = Vec::new();
        encode_chunk(b"hello", &mut out);
        assert_eq!(out, b"5\r\nhello\r\n");
    }

    #[test]
    fn encode_chunk_uses_lowercase_hex_for_large_sizes() {
        // 0x100 = 256 バイト。デコーダは大文字・小文字いずれの hex も
        // 受理するが（is_ascii_hexdigit）、出力は決定的に小文字へ揃える。
        let data = vec![b'x'; 0x100];
        let mut out = Vec::new();
        encode_chunk(&data, &mut out);
        assert!(out.starts_with(b"100\r\n"));
    }

    #[test]
    fn encode_chunk_empty_data_is_noop() {
        let mut out = Vec::new();
        encode_chunk(b"", &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn encode_terminator_writes_last_chunk() {
        let mut out = Vec::new();
        encode_terminator(&mut out);
        assert_eq!(out, b"0\r\n\r\n");
    }

    #[test]
    fn encode_then_decode_roundtrip_single_chunk() {
        let mut out = Vec::new();
        encode_chunk(b"Wikipedia", &mut out);
        encode_terminator(&mut out);
        let mut decoded = Vec::new();
        let outcome = decode_all_into(&out, &mut decoded).unwrap();
        assert!(matches!(outcome, DecodeOutcome::Complete { .. }));
        assert_eq!(decoded, b"Wikipedia");
    }

    #[test]
    fn encode_then_decode_roundtrip_multiple_chunks() {
        let mut out = Vec::new();
        encode_chunk(b"foo", &mut out);
        encode_chunk(b"bar", &mut out);
        encode_chunk(b"baz", &mut out);
        encode_terminator(&mut out);
        let mut decoded = Vec::new();
        let outcome = decode_all_into(&out, &mut decoded).unwrap();
        assert!(matches!(outcome, DecodeOutcome::Complete { .. }));
        assert_eq!(decoded, b"foobarbaz");
    }

    #[test]
    fn encode_then_decode_roundtrip_empty_body() {
        // データチャンクを 1 つも送らず終端のみの場合（空ストリーミング応答）。
        let mut out = Vec::new();
        encode_terminator(&mut out);
        let mut decoded = Vec::new();
        let outcome = decode_all_into(&out, &mut decoded).unwrap();
        assert!(matches!(outcome, DecodeOutcome::Complete { .. }));
        assert!(decoded.is_empty());
    }

    /// テスト専用ヘルパー: `ChunkedDecoder::new()` で `input` 全体を一度に
    /// 復号する（`decode_all` は非公開でエラー型固定のため、成功系
    /// roundtrip 検証用に別名で用意する）。
    fn decode_all_into(input: &[u8], out: &mut Vec<u8>) -> Result<DecodeOutcome, ChunkedError> {
        let mut decoder = ChunkedDecoder::new();
        decoder.decode(input, out)
    }
}
