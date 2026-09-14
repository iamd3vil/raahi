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
            upstream_authority: None,
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
                    priority: 0,
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
                upstream_authority: None,
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
        upstream_authority: None,
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

fn application(name: &str, domain: &str) -> ApplicationSpec {
    ApplicationSpec {
        name: name.into(),
        domain: domain.into(),
        upstream_url: "http://127.0.0.1:3000".into(),
        https: false,
        hsts: false,
        allowed_cidrs: vec![],
    }
}

#[tokio::test]
async fn application_creates_complete_routing_configuration() {
    let store = Store::connect(&tmp_db()).await.unwrap();
    store
        .create_certificate(&CertificateSpec {
            name: "wildcard".into(),
            sni: vec!["*.example.com".into()],
            cert_pem: "certificate material".into(),
            key_pem: "key material".into(),
            acme_config: None,
        })
        .await
        .unwrap();
    let mut spec = application(" Photos ", "PHOTOS.example.com.");
    spec.https = true;
    spec.hsts = true;
    spec.allowed_cidrs = vec!["100.64.0.0/10".into()];
    let created = store.create_application(&spec).await.unwrap();
    assert_eq!(created.name, "Photos");
    assert_eq!(created.url, "https://photos.example.com");
    let route = store.get_route(created.route_id).await.unwrap().unwrap();
    assert_eq!(route.service_id, created.service_id);
    assert_eq!(route.hosts, vec!["photos.example.com"]);
    assert!(route.preserve_host && !route.strip_path && route.enabled);
    assert_eq!(
        store.list_targets_for(created.service_id).await.unwrap()[0].id,
        created.target_id
    );
    let plugins = store.list_plugins().await.unwrap();
    assert_eq!(plugins.len(), 3);
    assert!(plugins.iter().all(|p| p.route_id == Some(created.route_id)));
    assert!(plugins.iter().any(|p| p.plugin_type == PluginType::Redirect
        && p.config["http_only"] == true
        && p.config["status"] == 308));
    assert!(
        plugins
            .iter()
            .any(|p| p.plugin_type == PluginType::IpRestriction
                && p.config["allow"][0] == "100.64.0.0/10")
    );
    let snapshot = store.build_snapshot().await.unwrap();
    assert_eq!(snapshot.routes.len(), 1);
    assert_eq!(snapshot.services.len(), 1);
}

#[tokio::test]
async fn application_rolls_back_all_resources_on_late_failure() {
    let store = Store::connect(&tmp_db()).await.unwrap();
    sqlx::query("CREATE TRIGGER reject_application_plugin BEFORE INSERT ON plugins BEGIN SELECT RAISE(ABORT, 'test failure'); END").execute(store.pool()).await.unwrap();
    let mut spec = application("app", "app.example.com");
    spec.allowed_cidrs = vec!["127.0.0.1".into()];
    assert!(store.create_application(&spec).await.is_err());
    assert!(store.list_services().await.unwrap().is_empty());
    assert!(store.list_all_targets().await.unwrap().is_empty());
    assert!(store.list_routes().await.unwrap().is_empty());
    assert!(store.list_plugins().await.unwrap().is_empty());
}

#[tokio::test]
async fn application_requires_certificates_and_rejects_duplicate_domains() {
    let store = Store::connect(&tmp_db()).await.unwrap();
    let mut spec = application("app", "app.example.com");
    spec.https = true;
    assert!(matches!(
        store.create_application(&spec).await,
        Err(raahi_store::StoreError::Invalid(_))
    ));
    assert!(store.list_services().await.unwrap().is_empty());
    spec.https = false;
    store.create_application(&spec).await.unwrap();
    spec.name = "another".into();
    assert!(matches!(
        store.create_application(&spec).await,
        Err(raahi_store::StoreError::Conflict(_))
    ));
    assert_eq!(store.list_services().await.unwrap().len(), 1);
    assert_eq!(store.list_routes().await.unwrap().len(), 1);
}

#[tokio::test]
async fn concurrent_application_submissions_cannot_duplicate_a_domain() {
    let store = Store::connect(&tmp_db()).await.unwrap();
    let first = application("first", "app.example.com");
    let second = application("second", "app.example.com");
    let (a, b) = tokio::join!(
        store.create_application(&first),
        store.create_application(&second)
    );
    assert_ne!(a.is_ok(), b.is_ok());
    assert_eq!(store.list_services().await.unwrap().len(), 1);
    assert_eq!(store.list_routes().await.unwrap().len(), 1);
    assert_eq!(store.list_all_targets().await.unwrap().len(), 1);
}

#[tokio::test]
async fn application_rejects_higher_priority_wildcard_routes() {
    let store = Store::connect(&tmp_db()).await.unwrap();
    let existing = store
        .create_application(&application("first", "first.example.com"))
        .await
        .unwrap();
    sqlx::query("UPDATE routes SET hosts='[\"*.example.com\"]', priority=200 WHERE id=?")
        .bind(existing.route_id)
        .execute(store.pool())
        .await
        .unwrap();
    let error = store
        .create_application(&application("second", "second.example.com"))
        .await
        .unwrap_err();
    assert!(matches!(error, raahi_store::StoreError::Conflict(_)));
    assert_eq!(store.list_services().await.unwrap().len(), 1);
}

#[tokio::test]
async fn eab_secrets_are_scoped_by_directory_and_survive_backup_restore() {
    let store = Store::connect(&tmp_db()).await.unwrap();
    let secret = AcmeEabCredentials {
        directory_url: ZEROSSL_DIRECTORY.into(),
        key_id: "kid".into(),
        hmac_key: "c2VjcmV0".into(),
    };
    store.set_acme_eab(&secret).await.unwrap();
    assert!(
        store
            .get_acme_eab(LETS_ENCRYPT_PRODUCTION)
            .await
            .unwrap()
            .is_none()
    );
    let backup = serde_json::json!({"raahi_export_version":1,"acme":{"eab_credentials":store.list_acme_eab().await.unwrap()}});
    store.delete_acme_eab(ZEROSSL_DIRECTORY).await.unwrap();
    assert!(
        store
            .get_acme_eab(ZEROSSL_DIRECTORY)
            .await
            .unwrap()
            .is_none()
    );
    store
        .import(&serde_json::from_value(backup).unwrap())
        .await
        .unwrap();
    assert_eq!(
        store
            .get_acme_eab(ZEROSSL_DIRECTORY)
            .await
            .unwrap()
            .unwrap()
            .hmac_key,
        "c2VjcmV0"
    );
    let old_backup: ImportDoc = serde_json::from_value(
        serde_json::json!({"raahi_export_version":1,"acme":{"accounts":[]}}),
    )
    .unwrap();
    store.import(&old_backup).await.unwrap();
    assert!(store.list_acme_eab().await.unwrap().is_empty());
}

#[tokio::test]
async fn discovery_reconciliation_preserves_ids_and_drains_missing_targets() {
    use std::collections::BTreeMap;
    let url = tmp_db();
    let store = Store::connect(&url).await.unwrap();
    let service = store
        .create_service(&ServiceSpec {
            name: "discovered".into(),
            protocol: Protocol::Http,
            connect_timeout_ms: 1000,
            read_timeout_ms: 1000,
            write_timeout_ms: 1000,
            retries: 0,
            lb_algorithm: LbAlgorithm::RoundRobin,
            upstream_authority: Some("api.internal".into()),
            tls_sni: None,
            health_path: None,
        })
        .await
        .unwrap();
    let source = store
        .create_discovery_source(
            service.id,
            &DiscoverySourceSpec {
                name: "registry".into(),
                provider: "http".into(),
                config: serde_json::json!({"url":"http://registry.test"}),
                enabled: true,
                stale_after_ms: 1000,
                removal_grace_ms: 60_000,
            },
        )
        .await
        .unwrap();
    let endpoint = DiscoveredEndpoint {
        key: "a".into(),
        host: "10.0.0.1".into(),
        port: 8080,
        weight: 20,
        priority: 5,
        metadata: BTreeMap::from([("zone".into(), "a".into())]),
    };
    store
        .reconcile_discovery(
            &source,
            std::slice::from_ref(&endpoint),
            "1",
            chrono::Utc::now(),
        )
        .await
        .unwrap();
    let first = store.list_targets_for(service.id).await.unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].priority, 5);
    assert_eq!(first[0].source_id, Some(source.id));
    assert_eq!(first[0].state, TargetState::Active);

    store
        .reconcile_discovery(
            &source,
            std::slice::from_ref(&endpoint),
            "1",
            chrono::Utc::now(),
        )
        .await
        .unwrap();
    assert_eq!(
        store.list_targets_for(service.id).await.unwrap()[0].id,
        first[0].id
    );

    store
        .reconcile_discovery(&source, &[], "2", chrono::Utc::now())
        .await
        .unwrap();
    let draining = store.list_targets_for(service.id).await.unwrap();
    assert_eq!(draining[0].state, TargetState::Draining);
    assert!(
        !store
            .build_snapshot()
            .await
            .unwrap()
            .targets
            .contains_key(&service.id)
    );
}

#[tokio::test]
async fn discovery_failure_marks_targets_stale_and_unusable() {
    use std::collections::BTreeMap;
    let url = tmp_db();
    let store = Store::connect(&url).await.unwrap();
    let service = store
        .create_service(&ServiceSpec {
            name: "stale".into(),
            protocol: Protocol::Http,
            connect_timeout_ms: 1000,
            read_timeout_ms: 1000,
            write_timeout_ms: 1000,
            retries: 0,
            lb_algorithm: LbAlgorithm::RoundRobin,
            upstream_authority: None,
            tls_sni: None,
            health_path: None,
        })
        .await
        .unwrap();
    let source = store
        .create_discovery_source(
            service.id,
            &DiscoverySourceSpec {
                name: "dns".into(),
                provider: "dns".into(),
                config: serde_json::json!({"hostname":"api.test","port":80}),
                enabled: true,
                stale_after_ms: 0,
                removal_grace_ms: 0,
            },
        )
        .await
        .unwrap();
    store
        .reconcile_discovery(
            &source,
            &[DiscoveredEndpoint {
                key: "a".into(),
                host: "10.0.0.1".into(),
                port: 80,
                weight: 100,
                priority: 0,
                metadata: BTreeMap::new(),
            }],
            "1",
            chrono::Utc::now(),
        )
        .await
        .unwrap();
    assert!(
        store
            .record_discovery_failure(&source, "dns down", chrono::Utc::now())
            .await
            .unwrap()
    );
    let target = &store.list_targets_for(service.id).await.unwrap()[0];
    assert_eq!(target.state, TargetState::Stale);
    assert!(!target.enabled);
}
