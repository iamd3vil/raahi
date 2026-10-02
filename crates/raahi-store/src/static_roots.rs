//! Guard against accidentally publishing Raahi's database or system files.
//! These configuration checks are not the traversal boundary: confined file
//! opens enforce that, and root paths must remain under trusted local control.

use std::path::{Component, Path, PathBuf};

use raahi_core::{ServiceKind, ServiceSpec};

use crate::{Store, StoreError};

impl Store {
    pub(crate) async fn validate_service(&self, spec: &ServiceSpec) -> Result<(), StoreError> {
        spec.validate().map_err(StoreError::Invalid)?;
        if spec.kind != ServiceKind::Static {
            return Ok(());
        }
        let root = spec.root.clone().expect("validated static root");
        let database_dir = self.database_dir.clone();
        tokio::task::spawn_blocking(move || {
            let root = resolve_root(Path::new(&root))?;
            if root.parent().is_none()
                || ["/proc", "/sys", "/dev"]
                    .iter()
                    .any(|p| root.starts_with(p))
            {
                return Err(StoreError::Invalid(
                    "static root cannot be the filesystem root or lie under /proc, /sys or /dev"
                        .into(),
                ));
            }
            if database_dir
                .as_ref()
                .is_some_and(|db| db.starts_with(&root))
            {
                return Err(StoreError::Invalid(
                    "static root cannot contain Raahi's database directory".into(),
                ));
            }
            Ok(())
        })
        .await
        .map_err(|e| StoreError::Invalid(format!("static root validation failed: {e}")))?
    }
}

/// Resolve symlinks in existing components, preserving the ability to configure
/// a site before deploying its directory. Process `..` after symlink resolution,
/// in filesystem order, rather than trusting a lexical string-prefix check.
fn resolve_root(path: &Path) -> Result<PathBuf, StoreError> {
    let mut resolved = PathBuf::new();
    for part in path.components() {
        match part {
            Component::Prefix(_) | Component::RootDir => resolved.push(part.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            Component::Normal(name) => {
                resolved.push(name);
                match std::fs::canonicalize(&resolved) {
                    Ok(path) => resolved = path,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        // A dangling symlink isn't a future ordinary directory.
                        if std::fs::symlink_metadata(&resolved)
                            .is_ok_and(|m| m.file_type().is_symlink())
                        {
                            return Err(StoreError::Invalid(
                                "static root contains a dangling symlink".into(),
                            ));
                        }
                    }
                    Err(e) => {
                        return Err(StoreError::Invalid(format!(
                            "cannot resolve static root: {e}"
                        )));
                    }
                }
            }
        }
    }
    Ok(resolved)
}
