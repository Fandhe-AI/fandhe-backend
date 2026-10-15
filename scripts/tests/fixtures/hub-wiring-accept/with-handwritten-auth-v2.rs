// フィクスチャ: マーカー区間はクリーンだが、ハンドラ（`build_router`）内で
// 汎用 JWT 検証クレート（plugin-jwt）の検証 API を直接呼んでいるケース
// （イシュー #820 以降。旧シンボル `verify_token` が消えた後も、配線をプラグインへ
// 集約できていないアンチパターンを判定 B 補足が検出できることを確認する）。

fn main() {
    // --- wiring:begin ---
    let config = TenantGateConfig::from_jwks_json(&jwks_json, policy).unwrap();
    let server = Server::new().gate(TenantGate::new(config));
    // --- wiring:end ---
}

fn build_router(store: Store) -> Router {
    let mut router = Router::new();
    router = router.route("GET", "/items", move |head, _body| {
        // アンチパターン: ハンドラ側で独自に検証 API を呼んでいる。
        let claims =
            jwt::verify::<HubClaims>(bearer_token(head).unwrap(), &keys, &policy, 0).unwrap();
        Response::new(200, claims.iss().as_bytes().to_vec())
    });
    router
}
