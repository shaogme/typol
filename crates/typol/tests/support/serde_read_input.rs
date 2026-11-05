use renamed_typol::{
    Environment, LoadedSchema, TypolError, ValidatedConfigValue,
    opeo::{OResult, Out},
};
use std::{io::Read as IoRead, path::Path};

#[cfg(feature = "serde")]
use renamed_typol::{BoundSchema, binding::Typol};

#[cfg(feature = "serde")]
use serde::de::DeserializeOwned;

pub enum TestSource<'input> {
    File {
        path: &'input Path,
        display_name: &'input str,
    },
    Reader {
        reader: &'input mut dyn IoRead,
        display_name: &'input str,
    },
}

impl<'input> TestSource<'input> {
    pub fn file(path: &'input Path) -> Self {
        Self::File {
            path,
            display_name: "config",
        }
    }

    pub fn reader(reader: &'input mut dyn IoRead) -> Self {
        Self::Reader {
            reader,
            display_name: "config",
        }
    }

    pub fn named(mut self, display_name: &'input str) -> Self {
        match &mut self {
            Self::File {
                display_name: name,
                ..
            }
            | Self::Reader {
                display_name: name,
                ..
            } => *name = display_name,
        }
        self
    }

    pub fn read_value<'slot>(
        self,
        loaded: &LoadedSchema,
        environment: &mut dyn Environment,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, ValidatedConfigValue> {
        match self {
            Self::File { path, display_name } => {
                loaded.read_file_value_named(path, display_name, environment, out)
            }
            Self::Reader {
                reader,
                display_name,
            } => loaded.read_io(reader, display_name, environment, out),
        }
    }

    #[cfg(feature = "serde")]
    pub fn read_serde<'slot, T: Typol + DeserializeOwned>(
        self,
        bound: &BoundSchema<'_, T>,
        environment: &mut dyn Environment,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, T> {
        match self {
            Self::File { path, display_name } => {
                bound.read_serde_file_named(path, display_name, environment, out)
            }
            Self::Reader {
                reader,
                display_name,
            } => bound.read_serde_io(reader, display_name, environment, out),
        }
    }
}
