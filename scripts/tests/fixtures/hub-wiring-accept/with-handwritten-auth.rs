// フィクスチャ: マーカー区間はクリーンだが、ハンドラ（`build_router`）内に
// 手書き JWT 検証シンボル（`verify_token`）が混入しているケース（判定 B補足が
// これを検出できることを確認する）。

fn main() {
    // --- wiring:begin ---
    let config = TenantGateConfig::from_jwks_json(&jwks_json).unwrap();
    let server = Server::new().gate(TenantGate::new(config));
    // --- wiring:end ---
}

fn build_router(store: Store) -> Router {
    let mut router = Router::new();
    router = router.route("GET", "/items", move |head, _body| {
        // アンチパターン: ハンドラ側で独自に JWT を再検証している（配線がプラグインへ
        // 集約できていない証拠）。
        let claims = verify_token(bearer_token(head).unwrap(), &keys, 0).unwrap();
        Response::new(200, claims.org_id.into_bytes())
    });
    router
}
