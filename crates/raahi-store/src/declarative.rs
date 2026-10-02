//! Declarative apply: reconcile the store against a name-keyed [`ConfigDoc`] in
//! one transaction, and dump the current state back into one.
//!
//! Entities are matched by name (targets by host:port, plugins by type and
//! position within their owner), so unchanged entities keep their ids and runtime
//! state — discovered targets, health, issued ACME certificates.

use std::collections::{HashMap, HashSet};

use raahi_core::*;
use serde::Serialize;
use sqlx::SqliteConnection;

use crate::crud::map_err;
use crate::discovery::{insert_discovery_source, update_discovery_source_in};
use crate::rows::*;
use crate::{Store, StoreError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeAction {
    Create,
    Update,
    Delete,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConfigChange {
    pub action: ChangeAction,
    pub kind: &'static str,
    pub name: String,
}

/// What an apply changed (or, for a dry run, would change).
#[derive(Debug, Default, Serialize)]
pub struct ApplyReport {
    pub dry_run: bool,
    pub changes: Vec<ConfigChange>,
}

impl ApplyReport {
    fn push(&mut self, action: ChangeAction, kind: &'static str, name: impl Into<String>) {
        self.changes.push(ConfigChange {
            action,
            kind,
            name: name.into(),
        });
    }

    pub fn touched(&self, kind: &str) -> bool {
        self.changes.iter().any(|c| c.kind == kind)
    }
}

use ChangeAction::{Create, Delete, Update};

impl Store {
    /// Reconcile the store against `doc` in one transaction; a dry run rolls it
    /// back, so the plan it reports is exactly what a real apply would do.
    ///
    /// Callers validate entity contents first (plugin configs, PEMs, discovery
    /// providers) and pass credential secrets in stored form, as for
    /// [`Store::create_credential`]. An omitted credential secret or certificate
    /// PEM keeps the stored one.
    pub async fn apply_config(
        &self,
        doc: &ConfigDoc,
        dry_run: bool,
    ) -> Result<ApplyReport, StoreError> {
        check_doc(doc)?;
        for service in doc.services.iter().flatten() {
            self.validate_service(&service.spec()).await?;
            if service.kind == ServiceKind::Static
                && (!service.targets.is_empty() || !service.discovery.is_empty())
            {
                return Err(StoreError::Invalid(
                    "static services cannot have targets or discovery sources".into(),
                ));
            }
        }
        let mut report = ApplyReport {
            dry_run,
            ..Default::default()
        };
        let mut tx = self.pool.begin().await?;

        let current_services: Vec<Service> = sqlx::query("SELECT * FROM services ORDER BY id")
            .fetch_all(&mut *tx)
            .await?
            .iter()
            .map(map_service)
            .collect();
        let mut svc_ids: HashMap<String, Id> = current_services
            .iter()
            .map(|s| (s.name.clone(), s.id))
            .collect();
        // Defer proxy -> static switches until declared stream routes have been
        // deleted or moved. First create/update all other services, so a stream
        // route can move to a newly created proxy in the same transaction.
        let converting: HashSet<&str> = doc
            .services
            .iter()
            .flatten()
            .filter(|s| {
                s.kind == ServiceKind::Static
                    && current_services
                        .iter()
                        .any(|e| e.name == s.name && e.kind == ServiceKind::Proxy)
            })
            .map(|s| s.name.as_str())
            .collect();

        if let Some(services) = &doc.services {
            for cs in services {
                if converting.contains(cs.name.as_str()) {
                    continue;
                }
                let id = upsert_service(&mut tx, &current_services, cs, &mut report).await?;
                svc_ids.insert(cs.name.clone(), id);
                apply_targets(&mut tx, id, &cs.name, &cs.targets, &mut report).await?;
                apply_discovery(&mut tx, id, &cs.name, &cs.discovery, &mut report).await?;
            }
            // Services missing from the file are deleted at the end; nothing in the
            // file may reference them.
            svc_ids.retain(|name, _| services.iter().any(|s| &s.name == name));
        }
        if let Some(stream_routes) = &doc.stream_routes {
            apply_stream_routes(&mut tx, stream_routes, &svc_ids, &mut report).await?;
        }
        for cs in doc
            .services
            .iter()
            .flatten()
            .filter(|s| converting.contains(s.name.as_str()))
        {
            let id = svc_ids[&cs.name];
            // Nested lists are fully managed; for static services they must be
            // empty. Delete upstream state before the kind-update trigger runs.
            apply_targets(&mut tx, id, &cs.name, &[], &mut report).await?;
            apply_discovery(&mut tx, id, &cs.name, &[], &mut report).await?;
            upsert_service(&mut tx, &current_services, cs, &mut report).await?;
        }
        if let Some(certs) = &doc.certificates {
            apply_certificates(&mut tx, certs, &mut report).await?;
        }
        let mut route_ids = HashMap::new();
        if let Some(routes) = &doc.routes {
            route_ids = apply_routes(&mut tx, routes, &svc_ids, &mut report).await?;
        }
        if let Some(plugins) = &doc.plugins {
            apply_plugins(&mut tx, Owner::Global, plugins, &mut report).await?;
        }
        if let Some(services) = &doc.services {
            for cs in services {
                let owner = Owner::Service(svc_ids[&cs.name], &cs.name);
                apply_plugins(&mut tx, owner, &cs.plugins, &mut report).await?;
            }
        }
        if let Some(routes) = &doc.routes {
            for r in routes {
                let owner = Owner::Route(route_ids[&r.name], &r.name);
                apply_plugins(&mut tx, owner, &r.plugins, &mut report).await?;
            }
        }
        if let Some(consumers) = &doc.consumers {
            apply_consumers(&mut tx, consumers, &mut report).await?;
        }
        if let Some(services) = &doc.services {
            delete_undeclared_services(&mut tx, &current_services, services, &mut report).await?;
        }

        if dry_run {
            tx.rollback().await?;
        } else {
            tx.commit().await?;
        }
        Ok(report)
    }

    /// The current state as a config document with every section present.
    /// Credential secrets and certificate PEMs are left out (apply keeps the
    /// stored ones when they are omitted).
    pub async fn current_config(&self) -> Result<ConfigDoc, StoreError> {
        let services = self.list_services().await?;
        let targets = self.list_all_targets().await?;
        let sources = self.list_discovery_sources().await?;
        // Id order (not `ordering`) so a dump re-applies as a no-op: apply pairs
        // same-type plugins by position in id order.
        let mut plugins = self.list_plugins().await?;
        plugins.sort_by_key(|p| p.id);
        let routes = self.list_routes().await?;
        let names: HashMap<Id, &str> = services.iter().map(|s| (s.id, s.name.as_str())).collect();
        let name_of = |id: Id| names.get(&id).map(|n| n.to_string()).unwrap_or_default();
        let plugins_where = |pred: &dyn Fn(&Plugin) -> bool| -> Vec<ConfigPlugin> {
            plugins
                .iter()
                .filter(|p| pred(p))
                .map(config_plugin)
                .collect()
        };

        let mut consumers = Vec::new();
        let credentials = self.list_all_credentials().await?;
        for c in self.list_consumers().await? {
            consumers.push(ConfigConsumer {
                credentials: credentials
                    .iter()
                    .filter(|cr| cr.consumer_id == c.id)
                    .map(|cr| ConfigCredential {
                        credential_type: cr.credential_type,
                        identifier: cr.identifier.clone(),
                        secret: None,
                        algorithm: None,
                    })
                    .collect(),
                username: c.username,
                groups: c.groups,
            });
        }

        Ok(ConfigDoc {
            raahi_config: CONFIG_VERSION,
            services: Some(
                services
                    .iter()
                    .map(|s| {
                        let spec = service_spec(s);
                        ConfigService {
                            name: spec.name,
                            kind: spec.kind,
                            root: spec.root,
                            spa_fallback: spec.spa_fallback,
                            protocol: spec.protocol,
                            connect_timeout_ms: spec.connect_timeout_ms,
                            read_timeout_ms: spec.read_timeout_ms,
                            write_timeout_ms: spec.write_timeout_ms,
                            retries: spec.retries,
                            lb_algorithm: spec.lb_algorithm,
                            upstream_authority: spec.upstream_authority,
                            tls_sni: spec.tls_sni,
                            health_path: spec.health_path,
                            targets: targets
                                .iter()
                                .filter(|t| t.service_id == s.id && t.source_id.is_none())
                                .map(|t| ConfigTarget {
                                    host: t.host.clone(),
                                    port: t.port,
                                    weight: t.weight,
                                    priority: t.priority,
                                    enabled: t.enabled,
                                })
                                .collect(),
                            discovery: sources
                                .iter()
                                .filter(|d| d.service_id == s.id)
                                .map(|d| ConfigDiscoverySource {
                                    name: d.name.clone(),
                                    provider: d.provider.clone(),
                                    config: d.config.clone(),
                                    enabled: d.enabled,
                                    stale_after_ms: d.stale_after_ms,
                                    removal_grace_ms: d.removal_grace_ms,
                                })
                                .collect(),
                            plugins: plugins_where(&|p| {
                                p.scope == PluginScope::Service && p.service_id == Some(s.id)
                            }),
                        }
                    })
                    .collect(),
            ),
            routes: Some(
                routes
                    .iter()
                    .map(|r| ConfigRoute {
                        name: r.name.clone(),
                        service: name_of(r.service_id),
                        priority: r.priority,
                        hosts: r.hosts.clone(),
                        paths: r.paths.clone(),
                        methods: r.methods.clone(),
                        headers: r.headers.clone(),
                        splits: r
                            .splits
                            .iter()
                            .map(|sp| ConfigSplit {
                                service: name_of(sp.service_id),
                                weight: sp.weight,
                            })
                            .collect(),
                        strip_path: r.strip_path,
                        preserve_host: r.preserve_host,
                        enabled: r.enabled,
                        plugins: plugins_where(&|p| {
                            p.scope == PluginScope::Route && p.route_id == Some(r.id)
                        }),
                    })
                    .collect(),
            ),
            stream_routes: Some(
                self.list_stream_routes()
                    .await?
                    .into_iter()
                    .map(|sr| ConfigStreamRoute {
                        service: name_of(sr.service_id),
                        name: sr.name,
                        listen_addr: sr.listen_addr,
                        enabled: sr.enabled,
                    })
                    .collect(),
            ),
            plugins: Some(plugins_where(&|p| p.scope == PluginScope::Global)),
            consumers: Some(consumers),
            certificates: Some(
                self.list_certificates()
                    .await?
                    .into_iter()
                    .map(|c| ConfigCertificate {
                        name: c.name,
                        sni: c.sni,
                        cert_pem: None,
                        key_pem: None,
                        acme_config: c.acme_config,
                    })
                    .collect(),
            ),
        })
    }
}

/// Version check plus duplicate keys, which would otherwise silently collapse.
fn check_doc(doc: &ConfigDoc) -> Result<(), StoreError> {
    if doc.raahi_config != CONFIG_VERSION {
        return Err(StoreError::Invalid(format!(
            "unsupported raahi_config version {} (expected {CONFIG_VERSION})",
            doc.raahi_config
        )));
    }
    fn unique(what: &str, keys: impl IntoIterator<Item = String>) -> Result<(), StoreError> {
        let mut seen = HashSet::new();
        for key in keys {
            if key.trim().is_empty() {
                return Err(StoreError::Invalid(format!("{what}: name is required")));
            }
            if !seen.insert(key.clone()) {
                return Err(StoreError::Invalid(format!("duplicate {what} '{key}'")));
            }
        }
        Ok(())
    }
    for s in doc.services.iter().flatten() {
        let what = format!("target in service '{}'", s.name);
        unique(
            &what,
            s.targets.iter().map(|t| format!("{}:{}", t.host, t.port)),
        )?;
        let what = format!("discovery source in service '{}'", s.name);
        unique(&what, s.discovery.iter().map(|d| d.name.trim().to_string()))?;
    }
    unique(
        "service",
        doc.services.iter().flatten().map(|s| s.name.clone()),
    )?;
    unique("route", doc.routes.iter().flatten().map(|r| r.name.clone()))?;
    unique(
        "stream route",
        doc.stream_routes.iter().flatten().map(|r| r.name.clone()),
    )?;
    unique(
        "consumer",
        doc.consumers.iter().flatten().map(|c| c.username.clone()),
    )?;
    unique(
        "credential",
        doc.consumers
            .iter()
            .flatten()
            .flat_map(|c| &c.credentials)
            .map(|cr| format!("{} {}", cr.credential_type.as_str(), cr.identifier)),
    )?;
    unique(
        "certificate",
        doc.certificates.iter().flatten().map(|c| c.name.clone()),
    )?;
    Ok(())
}

fn service_spec(s: &Service) -> ServiceSpec {
    ServiceSpec {
        name: s.name.clone(),
        kind: s.kind,
        root: s.root.clone(),
        spa_fallback: s.spa_fallback,
        protocol: s.protocol,
        connect_timeout_ms: s.connect_timeout_ms,
        read_timeout_ms: s.read_timeout_ms,
        write_timeout_ms: s.write_timeout_ms,
        retries: s.retries,
        lb_algorithm: s.lb_algorithm,
        upstream_authority: s.upstream_authority.clone(),
        tls_sni: s.tls_sni.clone(),
        health_path: s.health_path.clone(),
    }
}

fn config_plugin(p: &Plugin) -> ConfigPlugin {
    ConfigPlugin {
        plugin_type: p.plugin_type,
        config: p.config.clone(),
        ordering: p.ordering,
        enabled: p.enabled,
    }
}

async fn upsert_service(
    conn: &mut SqliteConnection,
    current: &[Service],
    cs: &ConfigService,
    report: &mut ApplyReport,
) -> Result<Id, StoreError> {
    let spec = cs.spec();
    let existing = current.iter().find(|s| s.name == cs.name);
    if let Some(e) = existing
        && service_spec(e) == spec
    {
        return Ok(e.id);
    }
    let now = chrono::Utc::now().to_rfc3339();
    match existing {
        Some(e) => {
            bind_service(sqlx::query(
                "UPDATE services SET name=?, protocol=?, connect_timeout_ms=?, read_timeout_ms=?, \
                 write_timeout_ms=?, retries=?, lb_algorithm=?, upstream_authority=?, tls_sni=?, \
                 health_path=?, kind=?, root=?, spa_fallback=?, updated_at=? WHERE id=?",
            ), &spec, &now)
            .bind(e.id)
            .execute(&mut *conn)
            .await
            .map_err(map_err)?;
            report.push(Update, "service", &cs.name);
            Ok(e.id)
        }
        None => {
            let res = bind_service(sqlx::query(
                "INSERT INTO services (name, protocol, connect_timeout_ms, read_timeout_ms, \
                 write_timeout_ms, retries, lb_algorithm, upstream_authority, tls_sni, health_path, \
                 kind, root, spa_fallback, updated_at, created_at) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
            ), &spec, &now)
            .bind(&now)
            .execute(&mut *conn)
            .await
            .map_err(map_err)?;
            report.push(Create, "service", &cs.name);
            Ok(res.last_insert_rowid())
        }
    }
}

type Query<'q> = sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>;

/// Binds the service columns in the order shared by the INSERT and UPDATE above.
fn bind_service<'q>(q: Query<'q>, spec: &'q ServiceSpec, now: &'q str) -> Query<'q> {
    q.bind(&spec.name)
        .bind(spec.protocol.as_str())
        .bind(spec.connect_timeout_ms as i64)
        .bind(spec.read_timeout_ms as i64)
        .bind(spec.write_timeout_ms as i64)
        .bind(spec.retries as i64)
        .bind(spec.lb_algorithm.as_str())
        .bind(&spec.upstream_authority)
        .bind(&spec.tls_sni)
        .bind(&spec.health_path)
        .bind(spec.kind.as_str())
        .bind(&spec.root)
        .bind(spec.spa_fallback)
        .bind(now)
}

/// Manual targets only, keyed by host:port; discovered targets are left alone.
async fn apply_targets(
    conn: &mut SqliteConnection,
    service_id: Id,
    service: &str,
    declared: &[ConfigTarget],
    report: &mut ApplyReport,
) -> Result<(), StoreError> {
    let existing: Vec<Target> =
        sqlx::query("SELECT * FROM targets WHERE service_id=? AND source_id IS NULL ORDER BY id")
            .bind(service_id)
            .fetch_all(&mut *conn)
            .await?
            .iter()
            .map(map_target)
            .collect();
    let label = |host: &str, port: u16| format!("{host}:{port} in {service}");
    for t in &existing {
        if !declared
            .iter()
            .any(|d| d.host == t.host && d.port == t.port)
        {
            sqlx::query("DELETE FROM targets WHERE id=?")
                .bind(t.id)
                .execute(&mut *conn)
                .await?;
            report.push(Delete, "target", label(&t.host, t.port));
        }
    }
    for d in declared {
        match existing
            .iter()
            .find(|t| t.host == d.host && t.port == d.port)
        {
            Some(t) if (t.weight, t.priority, t.enabled) == (d.weight, d.priority, d.enabled) => {}
            Some(t) => {
                sqlx::query("UPDATE targets SET weight=?, priority=?, enabled=? WHERE id=?")
                    .bind(d.weight as i64)
                    .bind(d.priority as i64)
                    .bind(d.enabled as i64)
                    .bind(t.id)
                    .execute(&mut *conn)
                    .await?;
                report.push(Update, "target", label(&d.host, d.port));
            }
            None => {
                sqlx::query(
                    "INSERT INTO targets (service_id, host, port, weight, priority, enabled) \
                     VALUES (?,?,?,?,?,?)",
                )
                .bind(service_id)
                .bind(&d.host)
                .bind(d.port as i64)
                .bind(d.weight as i64)
                .bind(d.priority as i64)
                .bind(d.enabled as i64)
                .execute(&mut *conn)
                .await
                .map_err(map_err)?;
                report.push(Create, "target", label(&d.host, d.port));
            }
        }
    }
    Ok(())
}

async fn apply_discovery(
    conn: &mut SqliteConnection,
    service_id: Id,
    service: &str,
    declared: &[ConfigDiscoverySource],
    report: &mut ApplyReport,
) -> Result<(), StoreError> {
    let existing: Vec<DiscoverySource> =
        sqlx::query("SELECT * FROM discovery_sources WHERE service_id=? ORDER BY id")
            .bind(service_id)
            .fetch_all(&mut *conn)
            .await?
            .iter()
            .map(map_discovery_source)
            .collect();
    let label = |name: &str| format!("{name} in {service}");
    for e in &existing {
        if !declared.iter().any(|d| d.name.trim() == e.name) {
            // Cascades to the targets it discovered.
            sqlx::query("DELETE FROM discovery_sources WHERE id=?")
                .bind(e.id)
                .execute(&mut *conn)
                .await?;
            report.push(Delete, "discovery_source", label(&e.name));
        }
    }
    for d in declared {
        let spec = DiscoverySourceSpec {
            name: d.name.trim().to_string(),
            ..d.spec()
        };
        match existing.iter().find(|e| e.name == spec.name) {
            Some(e) => {
                let current = DiscoverySourceSpec {
                    name: e.name.clone(),
                    provider: e.provider.clone(),
                    config: e.config.clone(),
                    enabled: e.enabled,
                    stale_after_ms: e.stale_after_ms,
                    removal_grace_ms: e.removal_grace_ms,
                };
                if current != spec {
                    update_discovery_source_in(conn, e.id, &spec).await?;
                    report.push(Update, "discovery_source", label(&spec.name));
                }
            }
            None => {
                insert_discovery_source(conn, service_id, &spec).await?;
                report.push(Create, "discovery_source", label(&spec.name));
            }
        }
    }
    Ok(())
}

async fn apply_certificates(
    conn: &mut SqliteConnection,
    declared: &[ConfigCertificate],
    report: &mut ApplyReport,
) -> Result<(), StoreError> {
    let existing: Vec<Certificate> = sqlx::query("SELECT * FROM certificates ORDER BY id")
        .fetch_all(&mut *conn)
        .await?
        .iter()
        .map(map_certificate)
        .collect();
    for e in &existing {
        if !declared.iter().any(|d| d.name == e.name) {
            sqlx::query("DELETE FROM certificates WHERE id=?")
                .bind(e.id)
                .execute(&mut *conn)
                .await?;
            report.push(Delete, "certificate", &e.name);
        }
    }
    for d in declared {
        let e = existing.iter().find(|e| e.name == d.name);
        let sni = d.sni.json();
        if let Some(acme) = &d.acme_config {
            // ACME: issuance state is runtime; only sni + acme_config are declared.
            // A change queues reissuance and keeps serving the old cert meanwhile.
            let pending = AcmeStatus::pending().json();
            match e {
                Some(e) if e.acme_config.as_ref() == Some(acme) && e.sni == d.sni => {}
                Some(e) => {
                    sqlx::query(
                        "UPDATE certificates SET sni=?, acme_config=?, acme_status=? WHERE id=?",
                    )
                    .bind(&sni)
                    .bind(acme.json())
                    .bind(&pending)
                    .bind(e.id)
                    .execute(&mut *conn)
                    .await?;
                    report.push(Update, "certificate", &d.name);
                }
                None => {
                    sqlx::query(
                        "INSERT INTO certificates (name, sni, cert_pem, key_pem, acme_config, \
                         acme_status) VALUES (?,?,'','',?,?)",
                    )
                    .bind(&d.name)
                    .bind(&sni)
                    .bind(acme.json())
                    .bind(&pending)
                    .execute(&mut *conn)
                    .await
                    .map_err(map_err)?;
                    report.push(Create, "certificate", &d.name);
                }
            }
            continue;
        }

        // Manual: omitted PEMs keep the stored ones, unless the stored ones were
        // ACME-issued (switching to manual must bring its own material).
        let kept = e.filter(|e| e.acme_config.is_none());
        let pem = |given: &Option<String>, stored: Option<&String>| {
            given
                .clone()
                .or_else(|| stored.cloned())
                .filter(|p| !p.is_empty())
        };
        let (Some(cert_pem), Some(key_pem)) = (
            pem(&d.cert_pem, kept.map(|e| &e.cert_pem)),
            pem(&d.key_pem, kept.map(|e| &e.key_pem)),
        ) else {
            return Err(StoreError::Invalid(format!(
                "certificate '{}' needs cert_pem and key_pem, or acme_config",
                d.name
            )));
        };
        match e {
            Some(e)
                if e.acme_config.is_none()
                    && e.sni == d.sni
                    && e.cert_pem == cert_pem
                    && e.key_pem == key_pem => {}
            Some(e) => {
                sqlx::query(
                    "UPDATE certificates SET sni=?, cert_pem=?, key_pem=?, acme_config=NULL, \
                     acme_status=NULL WHERE id=?",
                )
                .bind(&sni)
                .bind(&cert_pem)
                .bind(&key_pem)
                .bind(e.id)
                .execute(&mut *conn)
                .await?;
                report.push(Update, "certificate", &d.name);
            }
            None => {
                sqlx::query(
                    "INSERT INTO certificates (name, sni, cert_pem, key_pem) VALUES (?,?,?,?)",
                )
                .bind(&d.name)
                .bind(&sni)
                .bind(&cert_pem)
                .bind(&key_pem)
                .execute(&mut *conn)
                .await
                .map_err(map_err)?;
                report.push(Create, "certificate", &d.name);
            }
        }
    }
    Ok(())
}

fn resolve(svc_ids: &HashMap<String, Id>, name: &str, user: &str) -> Result<Id, StoreError> {
    svc_ids
        .get(name)
        .copied()
        .ok_or_else(|| StoreError::Invalid(format!("{user} references unknown service '{name}'")))
}

async fn apply_routes(
    conn: &mut SqliteConnection,
    declared: &[ConfigRoute],
    svc_ids: &HashMap<String, Id>,
    report: &mut ApplyReport,
) -> Result<HashMap<String, Id>, StoreError> {
    let existing: Vec<Route> = sqlx::query("SELECT * FROM routes ORDER BY id")
        .fetch_all(&mut *conn)
        .await?
        .iter()
        .map(map_route)
        .collect();
    for e in &existing {
        if !declared.iter().any(|d| d.name == e.name) {
            // Cascades to the route's plugins.
            sqlx::query("DELETE FROM routes WHERE id=?")
                .bind(e.id)
                .execute(&mut *conn)
                .await?;
            report.push(Delete, "route", &e.name);
        }
    }
    let mut ids = HashMap::new();
    for d in declared {
        let user = format!("route '{}'", d.name);
        let mut splits = Vec::new();
        for sp in &d.splits {
            splits.push(RouteSplit {
                service_id: resolve(svc_ids, &sp.service, &user)?,
                weight: sp.weight,
            });
        }
        let spec = RouteSpec {
            name: d.name.clone(),
            service_id: resolve(svc_ids, &d.service, &user)?,
            priority: d.priority,
            hosts: d.hosts.clone(),
            paths: d.paths.clone(),
            methods: d.methods.clone(),
            headers: d.headers.clone(),
            splits,
            strip_path: d.strip_path,
            preserve_host: d.preserve_host,
            enabled: d.enabled,
        };
        let e = existing.iter().find(|e| e.name == d.name);
        if let Some(e) = e {
            let current = RouteSpec {
                name: e.name.clone(),
                service_id: e.service_id,
                priority: e.priority,
                hosts: e.hosts.clone(),
                paths: e.paths.clone(),
                methods: e.methods.clone(),
                headers: e.headers.clone(),
                splits: e.splits.clone(),
                strip_path: e.strip_path,
                preserve_host: e.preserve_host,
                enabled: e.enabled,
            };
            ids.insert(d.name.clone(), e.id);
            if current == spec {
                continue;
            }
        }
        match e {
            Some(e) => {
                bind_route(
                    sqlx::query(
                        "UPDATE routes SET name=?, service_id=?, priority=?, hosts=?, paths=?, \
                     methods=?, headers=?, splits=?, strip_path=?, preserve_host=?, enabled=? \
                     WHERE id=?",
                    ),
                    &spec,
                )
                .bind(e.id)
                .execute(&mut *conn)
                .await
                .map_err(map_err)?;
                report.push(Update, "route", &d.name);
            }
            None => {
                let res = bind_route(
                    sqlx::query(
                        "INSERT INTO routes (name, service_id, priority, hosts, paths, methods, \
                     headers, splits, strip_path, preserve_host, enabled) \
                     VALUES (?,?,?,?,?,?,?,?,?,?,?)",
                    ),
                    &spec,
                )
                .execute(&mut *conn)
                .await
                .map_err(map_err)?;
                ids.insert(d.name.clone(), res.last_insert_rowid());
                report.push(Create, "route", &d.name);
            }
        }
    }
    Ok(ids)
}

fn bind_route<'q>(q: Query<'q>, spec: &'q RouteSpec) -> Query<'q> {
    q.bind(&spec.name)
        .bind(spec.service_id)
        .bind(spec.priority as i64)
        .bind(spec.hosts.json())
        .bind(spec.paths.json())
        .bind(spec.methods.json())
        .bind(spec.headers.json())
        .bind(spec.splits.json())
        .bind(spec.strip_path as i64)
        .bind(spec.preserve_host as i64)
        .bind(spec.enabled as i64)
}

async fn apply_stream_routes(
    conn: &mut SqliteConnection,
    declared: &[ConfigStreamRoute],
    svc_ids: &HashMap<String, Id>,
    report: &mut ApplyReport,
) -> Result<(), StoreError> {
    let existing: Vec<StreamRoute> = sqlx::query("SELECT * FROM stream_routes ORDER BY id")
        .fetch_all(&mut *conn)
        .await?
        .iter()
        .map(map_stream_route)
        .collect();
    // Deletes first so a listen_addr can move to another stream route.
    for e in &existing {
        if !declared.iter().any(|d| d.name == e.name) {
            sqlx::query("DELETE FROM stream_routes WHERE id=?")
                .bind(e.id)
                .execute(&mut *conn)
                .await?;
            report.push(Delete, "stream_route", &e.name);
        }
    }
    for d in declared {
        let service_id = resolve(svc_ids, &d.service, &format!("stream route '{}'", d.name))?;
        match existing.iter().find(|e| e.name == d.name) {
            Some(e)
                if (e.listen_addr.as_str(), e.service_id, e.enabled)
                    == (d.listen_addr.as_str(), service_id, d.enabled) => {}
            Some(e) => {
                sqlx::query(
                    "UPDATE stream_routes SET listen_addr=?, service_id=?, enabled=? WHERE id=?",
                )
                .bind(&d.listen_addr)
                .bind(service_id)
                .bind(d.enabled as i64)
                .bind(e.id)
                .execute(&mut *conn)
                .await
                .map_err(map_err)?;
                report.push(Update, "stream_route", &d.name);
            }
            None => {
                sqlx::query(
                    "INSERT INTO stream_routes (name, listen_addr, service_id, enabled) \
                     VALUES (?,?,?,?)",
                )
                .bind(&d.name)
                .bind(&d.listen_addr)
                .bind(service_id)
                .bind(d.enabled as i64)
                .execute(&mut *conn)
                .await
                .map_err(map_err)?;
                report.push(Create, "stream_route", &d.name);
            }
        }
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Owner<'a> {
    Global,
    Service(Id, &'a str),
    Route(Id, &'a str),
}

impl Owner<'_> {
    fn label(self, plugin_type: PluginType) -> String {
        let t = plugin_type.as_str();
        match self {
            Owner::Global => format!("{t} (global)"),
            Owner::Service(_, name) => format!("{t} on service {name}"),
            Owner::Route(_, name) => format!("{t} on route {name}"),
        }
    }
}

/// Pair the n-th declared plugin of a type with the n-th existing one (id order)
/// of that type under the same owner; leftovers are created or deleted.
async fn apply_plugins(
    conn: &mut SqliteConnection,
    owner: Owner<'_>,
    declared: &[ConfigPlugin],
    report: &mut ApplyReport,
) -> Result<(), StoreError> {
    let (scope, service_id, route_id) = match owner {
        Owner::Global => (PluginScope::Global, None, None),
        Owner::Service(id, _) => (PluginScope::Service, Some(id), None),
        Owner::Route(id, _) => (PluginScope::Route, None, Some(id)),
    };
    let existing: Vec<Plugin> = sqlx::query(
        "SELECT * FROM plugins WHERE scope=? AND service_id IS ? AND route_id IS ? ORDER BY id",
    )
    .bind(scope.as_str())
    .bind(service_id)
    .bind(route_id)
    .fetch_all(&mut *conn)
    .await?
    .iter()
    .map(map_plugin)
    .collect();
    let mut unmatched: Vec<&Plugin> = existing.iter().collect();
    for d in declared {
        let found = unmatched
            .iter()
            .position(|p| p.plugin_type == d.plugin_type)
            .map(|i| unmatched.remove(i));
        match found {
            Some(p) if config_plugin(p) == *d => {}
            Some(p) => {
                sqlx::query("UPDATE plugins SET config=?, ordering=?, enabled=? WHERE id=?")
                    .bind(d.config.to_string())
                    .bind(d.ordering as i64)
                    .bind(d.enabled as i64)
                    .bind(p.id)
                    .execute(&mut *conn)
                    .await?;
                report.push(Update, "plugin", owner.label(d.plugin_type));
            }
            None => {
                sqlx::query(
                    "INSERT INTO plugins (type, scope, service_id, route_id, config, ordering, \
                     enabled) VALUES (?,?,?,?,?,?,?)",
                )
                .bind(d.plugin_type.as_str())
                .bind(scope.as_str())
                .bind(service_id)
                .bind(route_id)
                .bind(d.config.to_string())
                .bind(d.ordering as i64)
                .bind(d.enabled as i64)
                .execute(&mut *conn)
                .await?;
                report.push(Create, "plugin", owner.label(d.plugin_type));
            }
        }
    }
    for p in unmatched {
        sqlx::query("DELETE FROM plugins WHERE id=?")
            .bind(p.id)
            .execute(&mut *conn)
            .await?;
        report.push(Delete, "plugin", owner.label(p.plugin_type));
    }
    Ok(())
}

fn credential_label(credential_type: CredentialType, identifier: &str, username: &str) -> String {
    match credential_type {
        // A key-auth identifier is the API key itself; keep it out of reports.
        CredentialType::KeyAuth => format!("key-auth for {username}"),
        t => format!("{} {identifier} for {username}", t.as_str()),
    }
}

async fn apply_consumers(
    conn: &mut SqliteConnection,
    declared: &[ConfigConsumer],
    report: &mut ApplyReport,
) -> Result<(), StoreError> {
    let existing: Vec<Consumer> = sqlx::query("SELECT * FROM consumers ORDER BY id")
        .fetch_all(&mut *conn)
        .await?
        .iter()
        .map(map_consumer)
        .collect();
    let creds: Vec<ConsumerCredential> =
        sqlx::query("SELECT * FROM consumer_credentials ORDER BY id")
            .fetch_all(&mut *conn)
            .await?
            .iter()
            .map(map_credential)
            .collect();

    // Deletes first so a credential can move between consumers in one apply.
    for e in &existing {
        let Some(d) = declared.iter().find(|d| d.username == e.username) else {
            sqlx::query("DELETE FROM consumers WHERE id=?")
                .bind(e.id)
                .execute(&mut *conn)
                .await?;
            report.push(Delete, "consumer", &e.username);
            continue;
        };
        for cr in creds.iter().filter(|cr| cr.consumer_id == e.id) {
            let kept = d.credentials.iter().any(|dc| {
                dc.credential_type == cr.credential_type && dc.identifier == cr.identifier
            });
            if !kept {
                sqlx::query("DELETE FROM consumer_credentials WHERE id=?")
                    .bind(cr.id)
                    .execute(&mut *conn)
                    .await?;
                let label = credential_label(cr.credential_type, &cr.identifier, &e.username);
                report.push(Delete, "credential", label);
            }
        }
    }

    for d in declared {
        let e = existing.iter().find(|e| e.username == d.username);
        let groups = d.groups.json();
        let consumer_id = match e {
            Some(e) if e.groups == d.groups => e.id,
            Some(e) => {
                sqlx::query("UPDATE consumers SET groups=? WHERE id=?")
                    .bind(&groups)
                    .bind(e.id)
                    .execute(&mut *conn)
                    .await?;
                report.push(Update, "consumer", &d.username);
                e.id
            }
            None => {
                let res = sqlx::query("INSERT INTO consumers (username, groups) VALUES (?,?)")
                    .bind(&d.username)
                    .bind(&groups)
                    .execute(&mut *conn)
                    .await
                    .map_err(map_err)?;
                report.push(Create, "consumer", &d.username);
                res.last_insert_rowid()
            }
        };
        for dc in &d.credentials {
            let label = credential_label(dc.credential_type, &dc.identifier, &d.username);
            let current = creds.iter().find(|cr| {
                cr.consumer_id == consumer_id
                    && cr.credential_type == dc.credential_type
                    && cr.identifier == dc.identifier
            });
            let secret = match dc.credential_type {
                CredentialType::KeyAuth => None,
                _ => dc.secret.as_deref(),
            };
            match current {
                Some(cr) => {
                    if let Some(secret) = secret
                        && cr.secret.as_deref() != Some(secret)
                    {
                        sqlx::query("UPDATE consumer_credentials SET secret=? WHERE id=?")
                            .bind(secret)
                            .bind(cr.id)
                            .execute(&mut *conn)
                            .await?;
                        report.push(Update, "credential", label);
                    }
                }
                None => {
                    if secret.is_none() && dc.credential_type != CredentialType::KeyAuth {
                        return Err(StoreError::Invalid(format!("{label}: secret is required")));
                    }
                    sqlx::query(
                        "INSERT INTO consumer_credentials (consumer_id, type, identifier, secret) \
                         VALUES (?,?,?,?)",
                    )
                    .bind(consumer_id)
                    .bind(dc.credential_type.as_str())
                    .bind(&dc.identifier)
                    .bind(secret)
                    .execute(&mut *conn)
                    .await
                    .map_err(map_err)?;
                    report.push(Create, "credential", label);
                }
            }
        }
    }
    Ok(())
}

/// Runs last, after routes have been repointed. Refuses to cascade-delete routes
/// that the file doesn't manage.
async fn delete_undeclared_services(
    conn: &mut SqliteConnection,
    current: &[Service],
    declared: &[ConfigService],
    report: &mut ApplyReport,
) -> Result<(), StoreError> {
    let routes: Vec<Route> = sqlx::query("SELECT * FROM routes")
        .fetch_all(&mut *conn)
        .await?
        .iter()
        .map(map_route)
        .collect();
    let stream_routes: Vec<StreamRoute> = sqlx::query("SELECT * FROM stream_routes")
        .fetch_all(&mut *conn)
        .await?
        .iter()
        .map(map_stream_route)
        .collect();
    for s in current {
        if declared.iter().any(|d| d.name == s.name) {
            continue;
        }
        let user = routes
            .iter()
            .find(|r| r.service_id == s.id || r.splits.iter().any(|sp| sp.service_id == s.id))
            .map(|r| format!("route '{}'", r.name))
            .or_else(|| {
                stream_routes
                    .iter()
                    .find(|r| r.service_id == s.id)
                    .map(|r| format!("stream route '{}'", r.name))
            });
        if let Some(user) = user {
            return Err(StoreError::Invalid(format!(
                "service '{}' is not in the file but {user} still uses it",
                s.name
            )));
        }
        sqlx::query("DELETE FROM services WHERE id=?")
            .bind(s.id)
            .execute(&mut *conn)
            .await?;
        report.push(Delete, "service", &s.name);
    }
    Ok(())
}

/// JSON text for list/map columns.
trait JsonText {
    fn json(&self) -> String;
}

impl<T: Serialize> JsonText for T {
    fn json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}
