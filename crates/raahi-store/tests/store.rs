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
    let path = std::env::temp_dir()
        .join(format!("raahi-test-{}-{seq}-{nanos}.db", std::process::id()));
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
                &TargetSpec { host: "127.0.0.1".into(), port, weight: 100, enabled: true },
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
        .create_consumer(&ConsumerSpec { username: "alice".into(), groups: vec!["team-a".into()] })
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

    let m = snap.match_route("anything", "/api/users", "GET", &|_| None).expect("route match");
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
    assert!(matches!(err, raahi_store::StoreError::Conflict(_)), "got {err:?}");
    let _ = std::fs::remove_file(url.trim_start_matches("sqlite://"));
}
