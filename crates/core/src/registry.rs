//! The Drivers compiled into this binary (ADR-0003). The binary registers each
//! one with a single line; the core never names a Driver.
use std::collections::BTreeMap;
use std::sync::Arc;

use serde::Serialize;

use crate::driver::{Driver, FieldSpec};

/// Every available Driver, by id.
#[derive(Clone, Default)]
pub struct DriverRegistry {
    drivers: BTreeMap<&'static str, Arc<dyn Driver>>,
}

/// What the UI needs to know about a Driver to offer it.
#[derive(Debug, Clone, Serialize)]
pub struct DriverInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub fields: Vec<FieldSpec>,
}

impl DriverRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a Driver. A second Driver with the same id replaces the first.
    pub fn register(&mut self, driver: impl Driver) -> &mut Self {
        self.drivers.insert(driver.id(), Arc::new(driver));
        self
    }

    pub fn get(&self, id: &str) -> Option<Arc<dyn Driver>> {
        self.drivers.get(id).cloned()
    }

    pub fn list(&self) -> Vec<DriverInfo> {
        self.drivers
            .values()
            .map(|d| DriverInfo {
                id: d.id(),
                name: d.name(),
                fields: d.fields(),
            })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.drivers.is_empty()
    }
}

impl std::fmt::Debug for DriverRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.drivers.keys()).finish()
    }
}
