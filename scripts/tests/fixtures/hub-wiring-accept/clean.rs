// フィクスチャ: 配線区間が正しくマーカーで囲まれ、ハンドラに手書き配線シンボルが
// 現れない「良い」ケース（マーカー区間 3 行）。

fn main() {
    // --- wiring:begin ---
    let config = TenantGateConfig::from_jwks_json(&jwks_json).unwrap();
    let authenticator = config.authenticator();
    let server = Server::new().gate(TenantGate::new(config));
    // --- wiring:end ---
}

fn build_router(store: Store, authenticator: Authenticator) -> Router {
    let mut router = Router::new();
    router = router.route("GET", "/items", move |head, _body| {
        let org_id = require_org(&authenticator, head);
        Response::new(200, org_id.into_bytes())
    });
    router
}
