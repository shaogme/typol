mod bound;
mod config;
mod loader;

#[cfg(all(test, feature = "std"))]
mod tests;

pub use bound::{BoundSchema, TrustedBoundSchema};
pub use config::LoadedSchema;
pub(crate) use config::{ConfigInputOperation, ConfigInputStrategy, ConfigSourceRequest};
pub use loader::SchemaLoader;

use core::fmt::{Debug, Formatter, Result};
use opeo::{OResult, Out};
use typol_core::{ByteSource, SourceId};

#[cfg(feature = "std")]
use crate::error::ErrorSlot;
#[cfg(not(feature = "std"))]
use alloc::vec::Vec;
#[cfg(not(feature = "std"))]
use core::str::from_utf8;
#[cfg(feature = "std")]
use std::{io::Read, path::Path};
#[cfg(not(feature = "std"))]
use typol_schema::{SourceLoadedRequest, SourceObserver, SourceObserverError};
#[cfg(feature = "std")]
use typol_std::{
    FileConfigInput, HostError, HostErrorKind, SourceNameEditOptions,
    SourceNames as HostSourceNames,
};

#[cfg(feature = "std")]
pub(crate) struct PreparedConfig {
    pub(crate) source: FileConfigInput,
    pub(crate) slot: ErrorSlot,
}

pub(crate) enum ConfigInputKind<'input> {
    #[cfg(feature = "std")]
    File(&'input Path),
    Text(&'input str),
    Bytes(&'input mut dyn ByteSource),
    #[cfg(feature = "std")]
    Reader(&'input mut dyn Read),
}

pub struct ConfigInput<'input> {
    source: ConfigInputKind<'input>,
    display_name: Option<&'input str>,
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

/// The host controls reader termination and timeouts.
#[cfg(feature = "std")]
pub struct ReaderLoadRequest<'input> {
    reader: &'input mut dyn Read,
    display_name: &'input str,
}

#[cfg(feature = "std")]
impl<'input> ReaderLoadRequest<'input> {
    pub fn new(reader: &'input mut dyn Read, display_name: &'input str) -> Self {
        Self {
            reader,
            display_name,
        }
    }

    pub(crate) fn into_parts(self) -> (&'input mut dyn Read, &'input str) {
        (self.reader, self.display_name)
    }

    pub const fn display_name(&self) -> &'input str {
        self.display_name
    }
}

impl<'input> ConfigInput<'input> {
    #[cfg(feature = "std")]
    pub fn file(path: &'input Path) -> Self {
        Self {
            source: ConfigInputKind::File(path),
            display_name: None,
        }
    }

    pub fn text(text: &'input str) -> Self {
        Self {
            source: ConfigInputKind::Text(text),
            display_name: None,
        }
    }

    pub fn bytes(source: &'input mut dyn ByteSource) -> Self {
        Self {
            source: ConfigInputKind::Bytes(source),
            display_name: None,
        }
    }

    #[cfg(feature = "std")]
    pub fn reader(reader: &'input mut dyn Read) -> Self {
        Self {
            source: ConfigInputKind::Reader(reader),
            display_name: None,
        }
    }

    pub const fn named(mut self, display_name: &'input str) -> Self {
        self.display_name = Some(display_name);
        self
    }

    pub const fn display_name(&self) -> &'input str {
        match self.display_name {
            Some(name) => name,
            None => "config",
        }
    }

    pub(crate) fn into_parts(self) -> (ConfigInputKind<'input>, &'input str) {
        (self.source, self.display_name.unwrap_or("config"))
    }
}

#[cfg(feature = "std")]
impl Debug for ReaderLoadRequest<'_> {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> Result {
        formatter
            .debug_struct("ReaderLoadRequest")
            .finish_non_exhaustive()
    }
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
