//! Request/response header transforms: add (or overwrite) and remove headers.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::Effects;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct TransformCfg {
    /// Headers to set (overwriting existing values).
    pub add: BTreeMap<String, String>,
    /// Header names to remove.
    pub remove: Vec<String>,
}

impl TransformCfg {
    pub fn apply_request(&self, effects: &mut Effects) {
        for (k, v) in &self.add {
            effects.req_add.push((k.clone(), v.clone()));
        }
        for k in &self.remove {
            effects.req_remove.push(k.clone());
        }
    }

    pub fn apply_response(&self, effects: &mut Effects) {
        for (k, v) in &self.add {
            effects.resp_add.push((k.clone(), v.clone()));
        }
        for k in &self.remove {
            effects.resp_remove.push(k.clone());
        }
    }
}
