use raahi_core::*;
use raahi_store::Store;

/// Unique temp DB path per test run (no shared-cache footguns with `:memory:` pools).
/// A process-local counter guards against clock-resolution collisions when tests
/// run in parallel.
fn tmp_db() -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "raahi-test-{}-{seq}-{nanos}.db",
        std::process::id()
    ));
    format!("sqlite://{}", path.display())
}

#[tokio::test]
async fn crud_and_snapshot_roundtrip() {
    let url = tmp_db();
    let store = Store::connect(&url).await.expect("connect");

    // Service + two targets.
    let svc = store
        .create_service(&ServiceSpec {
            name: "api".into(),
            protocol: Protocol::Http,
            connect_timeout_ms: 1000,
            read_timeout_ms: 5000,
            write_timeout_ms: 5000,
            retries: 1,
            lb_algorithm: LbAlgorithm::RoundRobin,
            tls_sni: None,
            health_path: None,
        })
        .await
        .expect("create service");

    for port in [9001u16, 9002] {
        store
            .create_target(
                svc.id,
                &TargetSpec {
                    host: "127.0.0.1".into(),
                    port,
                    weight: 100,
                    enabled: true,
                },
            )
            .await
            .expect("create target");
    }

    // Route bound to the service.
    let route = store
        .create_route(&RouteSpec {
            name: "api-route".into(),
            service_id: svc.id,
            priority: 0,
            hosts: vec![],
            paths: vec!["/api".into()],
            methods: vec![],
            headers: Default::default(),
            splits: vec![],
            strip_path: true,
            preserve_host: false,
            enabled: true,
        })
        .await
        .expect("create route");

    // Consumer + key-auth credential.
    let consumer = store
        .create_consumer(&ConsumerSpec {
            username: "alice".into(),
            groups: vec!["team-a".into()],
        })
        .await
        .expect("create consumer");
    store
        .create_credential(consumer.id, CredentialType::KeyAuth, "secret-key-123", None)
        .await
        .expect("create credential");

    // Plugin scoped to the route.
    store
        .create_plugin(&PluginSpec {
            plugin_type: PluginType::KeyAuth,
            scope: PluginScope::Route,
            service_id: None,
            route_id: Some(route.id),
            config: serde_json::json!({}),
            ordering: 0,
            enabled: true,
        })
        .await
        .expect("create plugin");

    // Build the snapshot and assert it reflects everything.
    let snap = store.build_snapshot().await.expect("snapshot");
    assert_eq!(snap.services.len(), 1);
    assert_eq!(snap.targets.get(&svc.id).map(|v| v.len()), Some(2));
    assert_eq!(snap.key_index.get("secret-key-123"), Some(&consumer.id));

    let m = snap
        .match_route("anything", "/api/users", "GET", &|_| None)
        .expect("route match");
    assert_eq!(m.route.id, route.id);
    assert_eq!(m.service.id, svc.id);
    assert!(m.route.strip_path);

    let plugins = snap.plugins_for(route.id, svc.id);
    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0].plugin_type, PluginType::KeyAuth);

    // Update + delete semantics.
    let updated = store
        .update_service(
            svc.id,
            &ServiceSpec {
                name: "api2".into(),
                protocol: Protocol::Https,
                connect_timeout_ms: 1000,
                read_timeout_ms: 5000,
                write_timeout_ms: 5000,
                retries: 2,
                lb_algorithm: LbAlgorithm::Weighted,
                tls_sni: Some("api.internal".into()),
                health_path: Some("/healthz".into()),
            },
        )
        .await
        .expect("update")
        .expect("some");
    assert_eq!(updated.name, "api2");
    assert_eq!(updated.protocol, Protocol::Https);

    assert!(store.delete_route(route.id).await.unwrap());
    assert!(!store.delete_route(route.id).await.unwrap());

    // Deleting the service cascades to its targets.
    assert!(store.delete_service(svc.id).await.unwrap());
    assert!(store.list_all_targets().await.unwrap().is_empty());

    let _ = std::fs::remove_file(url.trim_start_matches("sqlite://"));
}

#[tokio::test]
async fn acme_certificate_and_secrets_roundtrip() {
    let store = Store::connect(&tmp_db()).await.expect("connect");
    let certificate = store
        .create_certificate(&CertificateSpec {
            name: "automatic".into(),
            sni: vec!["example.com".into()],
            cert_pem: String::new(),
            key_pem: String::new(),
            acme_config: Some(AcmeConfig {
                directory_url: "staging".into(),
                challenge: AcmeChallenge::Dns01,
                email: Some("ops@example.com".into()),
            }),
        })
        .await
        .expect("create certificate");

    assert_eq!(certificate.acme_status.unwrap().state, AcmeState::Pending);
    store
        .upsert_acme_account("staging", Some("ops@example.com"), "credentials")
        .await
        .expect("save account");
    assert_eq!(
        store.get_acme_account("staging").await.unwrap(),
        Some(("credentials".into(), Some("ops@example.com".into())))
    );
    store
        .set_cloudflare_api_token(Some("secret-token"))
        .await
        .expect("save token");
    assert_eq!(
        store.get_cloudflare_api_token().await.unwrap().as_deref(),
        Some("secret-token")
    );
}

#[tokio::test]
async fn unique_violation_maps_to_conflict() {
    let url = tmp_db();
    let store = Store::connect(&url).await.unwrap();
    let spec = ServiceSpec {
        name: "dup".into(),
        protocol: Protocol::Http,
        connect_timeout_ms: 1000,
        read_timeout_ms: 1000,
        write_timeout_ms: 1000,
        retries: 0,
        lb_algorithm: LbAlgorithm::RoundRobin,
        tls_sni: None,
        health_path: None,
    };
    store.create_service(&spec).await.unwrap();
    let err = store.create_service(&spec).await.unwrap_err();
    assert!(
        matches!(err, raahi_store::StoreError::Conflict(_)),
        "got {err:?}"
    );
    let _ = std::fs::remove_file(url.trim_start_matches("sqlite://"));
}

#[tokio::test]
async fn users_sessions_and_sso_config() {
    let store = Store::connect(&tmp_db()).await.expect("connect");
    assert_eq!(store.count_users().await.unwrap(), 0);

    let admin = store
        .create_user("Admin@Example.com", "Root", Role::Admin, Some("$2b$hash"))
        .await
        .expect("create admin");
    assert_eq!(admin.email, "admin@example.com", "emails are normalized");
    assert!(admin.has_password);
    assert!(
        store
            .create_user("admin@example.com", "", Role::Viewer, None)
            .await
            .is_err()
    );

    let viewer = store
        .create_user("v@example.com", "", Role::Viewer, None)
        .await
        .unwrap();
    assert!(!viewer.has_password);
    assert_eq!(store.count_users().await.unwrap(), 2);
    assert_eq!(store.count_admins().await.unwrap(), 1);
    assert_eq!(
        store
            .get_user_by_email("ADMIN@example.com")
            .await
            .unwrap()
            .map(|u| u.id),
        Some(admin.id)
    );

    // Update keeps the password hash when none is supplied.
    let updated = store
        .update_user(admin.id, "admin@example.com", "Renamed", Role::Admin, None)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(updated.name, "Renamed");
    assert_eq!(
        store
            .get_user_password_hash(admin.id)
            .await
            .unwrap()
            .as_deref(),
        Some("$2b$hash")
    );

    // Sessions: live, expired, and cascade on user delete.
    let expires = store
        .create_session("h1", viewer.id, chrono::Duration::hours(1))
        .await
        .unwrap();
    assert!(expires > chrono::Utc::now());
    assert_eq!(
        store.session_user("h1").await.unwrap().map(|u| u.id),
        Some(viewer.id)
    );
    store
        .create_session("h-expired", viewer.id, chrono::Duration::seconds(-5))
        .await
        .unwrap();
    assert!(store.session_user("h-expired").await.unwrap().is_none());
    assert!(store.session_user("nope").await.unwrap().is_none());
    assert!(store.delete_user(viewer.id).await.unwrap());
    assert!(store.session_user("h1").await.unwrap().is_none());

    // SSO config round-trips as JSON in settings.
    assert!(store.get_sso_config().await.unwrap().is_none());
    let cfg = SsoConfig {
        issuer: "https://idp.example".into(),
        client_id: "raahi".into(),
        client_secret: "s3cret".into(),
        label: "Example".into(),
        auto_provision_role: Some(Role::Editor),
        allowed_domains: vec!["example.com".into()],
    };
    store.set_sso_config(Some(&cfg)).await.unwrap();
    assert_eq!(store.get_sso_config().await.unwrap(), Some(cfg));
    store.set_sso_config(None).await.unwrap();
    assert!(store.get_sso_config().await.unwrap().is_none());
}
