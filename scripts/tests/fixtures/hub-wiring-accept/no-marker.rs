// フィクスチャ: 配線マーカーが存在しないケース（`count_wiring_loc` が 0 を返す
// ことを確認する。マーカー未設置＝検証不能を表す境界値）。

fn main() {
    let config = TenantGateConfig::from_jwks_json(&jwks_json).unwrap();
    let authenticator = config.authenticator();
    let server = Server::new().gate(TenantGate::new(config));
}

fn build_router(store: Store, authenticator: Authenticator) -> Router {
    let mut router = Router::new();
    router = router.route("GET", "/items", move |head, _body| {
        let org_id = require_org(&authenticator, head);
        Response::new(200, org_id.into_bytes())
    });
    router
}
