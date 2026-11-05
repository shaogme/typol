mod bound;
mod config;
mod loader;

#[cfg(all(test, feature = "std"))]
mod tests;

pub use bound::{BoundRead, BoundSchema, TrustedBoundSchema};
pub use config::LoadedSchema;
pub(crate) use config::{ConfigSourceRequest, ReadStrategy};
pub use loader::SchemaLoader;

use core::fmt::{Debug, Formatter, Result};
use opeo::{OResult, Out};
use typol_core::SourceId;

#[cfg(feature = "std")]
use crate::error::ErrorSlot;
#[cfg(not(feature = "std"))]
use alloc::vec::Vec;
#[cfg(not(feature = "std"))]
use core::str::from_utf8;
#[cfg(feature = "std")]
use std::path::Path;
#[cfg(not(feature = "std"))]
use typol_schema::{SourceLoadedRequest, SourceObserver, SourceObserverError};
#[cfg(feature = "std")]
use typol_std::{
    FileConfigInput, HostError, HostErrorKind, SourceNameEditOptions,
    SourceNames as HostSourceNames,
};

#[cfg(feature = "std")]
pub(crate) struct FileReadRequest<'a> {
    pub(crate) path: &'a Path,
    pub(crate) display_name: &'a str,
}

#[cfg(feature = "std")]
pub(crate) struct PreparedConfig {
    pub(crate) source: FileConfigInput,
    pub(crate) slot: ErrorSlot,
}

#[cfg(not(feature = "std"))]
struct SourceName {
    id: SourceId,
    identity: Vec<u8>,
}

#[cfg(feature = "std")]
pub struct SourceNames {
    inner: HostSourceNames,
}

#[cfg(not(feature = "std"))]
#[derive(Default)]
pub struct SourceNames {
    entries: Vec<SourceName>,
}

#[cfg(feature = "std")]
impl SourceNames {
    pub(crate) const fn from_host(inner: HostSourceNames) -> Self {
        Self { inner }
    }

    pub(crate) const fn host_names(&self) -> &HostSourceNames {
        &self.inner
    }

    pub fn name(&self, source_id: SourceId) -> Option<&str> {
        self.inner.name(source_id)
    }

    pub fn identity(&self, source_id: SourceId) -> Option<&[u8]> {
        self.inner.identity(source_id)
    }

    pub const fn failure(&self) -> Option<HostErrorKind> {
        self.inner.failure()
    }

    pub(crate) fn set_name<'slot>(
        &mut self,
        source_id: SourceId,
        name: &str,
        options: SourceNameEditOptions,
        out: Out<'slot, '_, HostError>,
    ) -> OResult<'slot, ()> {
        self.inner.set_name(source_id, name, options, out)
    }
}

#[cfg(not(feature = "std"))]
impl SourceNames {
    pub fn identity(&self, source_id: SourceId) -> Option<&[u8]> {
        self.entries
            .iter()
            .find(|entry| entry.id == source_id)
            .map(|entry| entry.identity.as_slice())
    }

    pub fn name(&self, source_id: SourceId) -> Option<&str> {
        from_utf8(self.identity(source_id)?).ok()
    }
}

#[cfg(not(feature = "std"))]
impl SourceObserver for SourceNames {
    fn source_loaded<'slot>(
        &mut self,
        request: SourceLoadedRequest<'_>,
        out: Out<'slot, '_, SourceObserverError>,
    ) -> OResult<'slot, ()> {
        if request.source_id.get() as usize >= request.context.resources().max_schema_files() {
            return Err(out.fail(SourceObserverError::CapacityExceeded));
        }
        let mut identity = Vec::new();
        if identity.try_reserve_exact(request.identity.len()).is_err()
            || self.entries.try_reserve(1).is_err()
        {
            return Err(out.fail(SourceObserverError::AllocationFailed));
        }
        identity.extend_from_slice(request.identity);
        self.entries.push(SourceName {
            id: request.source_id,
            identity,
        });
        Ok(())
    }
}

impl Debug for SourceNames {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> Result {
        formatter
            .debug_struct("SourceNames")
            .field("count", &self.len())
            .finish()
    }
}

impl SourceNames {
    #[cfg(feature = "std")]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    #[cfg(not(feature = "std"))]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
