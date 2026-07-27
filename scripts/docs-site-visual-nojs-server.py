#!/usr/bin/env python3
"""scripts/docs-site-visual-nojs-server.py

役割:
    scripts/docs-site-visual.sh から呼ばれる補助サーバ。ビルド済み docs サイトを
    `Content-Security-Policy: script-src 'none'` ヘッダ付きで配信し、JS 未到達
    環境（headless chromium の --blink-settings=scriptEnabled=false は無音失敗
    しうるため使わない）を再現する。撮影対象は N1/N2（受け入れ条件の観点8:
    「JS 到達不能時に検索窓・トグルが非表示のままレイアウトが成立」）。

呼び出し元との契約:
    引数はポート番号のみ。カレントディレクトリを配信ルートとする
    （scripts/docs-site-visual.sh 側で `cd "$root"` 済みの状態から起動される）。
    127.0.0.1 のみへ bind し外部公開しない（実装計画 §5 A01）。
"""

from __future__ import annotations

import http.server
import socketserver
import sys


class NoJsHTTPRequestHandler(http.server.SimpleHTTPRequestHandler):
    """CSP script-src 'none' を全レスポンスへ付与する配信ハンドラ。"""

    def end_headers(self) -> None:  # noqa: D401 - http.server の契約に従う
        self.send_header("Content-Security-Policy", "script-src 'none'")
        super().end_headers()

    def log_message(self, format: str, *args: object) -> None:
        # 撮影用の一時サーバであり、標準エラーへのアクセスログは不要
        # （scripts/docs-site-visual.sh 側で server-*.log にリダイレクト済み）。
        pass


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: docs-site-visual-nojs-server.py <port>", file=sys.stderr)
        return 2
    port = int(sys.argv[1])
    # 127.0.0.1 のみへ bind する（0.0.0.0 にしない、実装計画 §5 A01）。
    with socketserver.TCPServer(("127.0.0.1", port), NoJsHTTPRequestHandler) as httpd:
        httpd.serve_forever()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
