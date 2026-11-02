use crate::{
    TypolError, WriteOptions,
    binding_impl::{DeserializeRequest, TrustedTypedSchema, TypedSchema},
    descriptor::Typol,
    diagnostics::CodecErrors,
    error::{ErrorNames, ErrorSlot},
    loading::{
        ConfigInput, ConfigInputOperation, ConfigInputStrategy, ConfigSourceRequest, LoadedSchema,
    },
    options::CodecOptions,
};
use alloc::string::String;
use core::ops::Deref;
use opeo::{ErrSlot, OResult, Out};
use typol_config::{ConfigParseRequest, Environment};
use typol_semantic::CompiledSchema;

#[cfg(feature = "std")]
use super::loader::LoadSupport;

#[cfg(feature = "serde")]
use serde::{Serialize, de::DeserializeOwned};

/// A reusable binding for a type accepted by the library-managed budget path.
///
/// Caller-provided Serde implementations remain a separate user-code trust boundary.
/// Reusable library-managed budget binding, available only to types with the standard binding capability marker.
pub struct BoundSchema<'loaded, T: Typol> {
    loaded: &'loaded LoadedSchema,
    binding: TypedSchema<'loaded, T>,
}

/// A reusable binding for a handwritten or otherwise trusted `Typol` implementation.
///
/// Library-managed operations remain budgeted. The wrapped `Typol` callbacks are trusted Rust
/// code and can allocate, block, or compute outside those budgets; this type does not sandbox them.
/// Caller-provided Serde implementations remain a separate user-code trust boundary.
///
pub struct TrustedBoundSchema<'loaded, T: Typol> {
    inner: BoundSchema<'loaded, T>,
}

impl<'loaded, T: Typol> Deref for TrustedBoundSchema<'loaded, T> {
    type Target = BoundSchema<'loaded, T>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

struct TypolDecode<'bound, 'loaded, T: Typol> {
    bound: &'bound BoundSchema<'loaded, T>,
}

impl<T: Typol> ConfigInputStrategy for TypolDecode<'_, '_, T> {
    type Value = T;
    type Error = CodecErrors;

    fn convert<'slot, 'borrow>(
        &mut self,
        request: ConfigSourceRequest<'_, '_, '_, '_>,
        out: Out<'slot, 'borrow, Self::Error>,
    ) -> OResult<'slot, Self::Value> {
        self.bound.binding.deserialize_with_budget(
            DeserializeRequest {
                request: ConfigParseRequest::new(
                    request.source,
                    request.display_name,
                    self.bound.loaded.options().config(),
                ),
                environment: request.environment,
                limits: self.bound.loaded.options().conversion(),
                budget: request.budget,
            },
            out,
        )
    }

    fn finish_error(
        &mut self,
        slot: ErrorSlot,
        error: Self::Error,
        names: ErrorNames,
    ) -> TypolError {
        #[cfg(feature = "std")]
        if let Some(source_error) = error
            .config_parse_errors()
            .and_then(|errors| errors.source_read_error())
        {
            return LoadSupport::read_source_error(source_error);
        }
        slot.codec(error, names)
    }
}

#[cfg(feature = "serde")]
struct SerdeDecode<'bound, 'loaded, T: Typol> {
    bound: &'bound BoundSchema<'loaded, T>,
}

#[cfg(feature = "serde")]
impl<T: Typol + DeserializeOwned> ConfigInputStrategy for SerdeDecode<'_, '_, T> {
    type Value = T;
    type Error = CodecErrors;

    fn convert<'slot, 'borrow>(
        &mut self,
        request: ConfigSourceRequest<'_, '_, '_, '_>,
        out: Out<'slot, 'borrow, Self::Error>,
    ) -> OResult<'slot, Self::Value> {
        self.bound.binding.deserialize_serde_with_budget(
            DeserializeRequest {
                request: ConfigParseRequest::new(
                    request.source,
                    request.display_name,
                    self.bound.loaded.options().config(),
                ),
                environment: request.environment,
                limits: self.bound.loaded.options().conversion(),
                budget: request.budget,
            },
            out,
        )
    }

    fn finish_error(
        &mut self,
        slot: ErrorSlot,
        error: Self::Error,
        names: ErrorNames,
    ) -> TypolError {
        #[cfg(feature = "std")]
        if let Some(source_error) = error
            .config_parse_errors()
            .and_then(|errors| errors.source_read_error())
        {
            return LoadSupport::read_source_error(source_error);
        }
        slot.codec(error, names)
    }
}

impl<'loaded, T: Typol> BoundSchema<'loaded, T> {
    pub(crate) fn new(loaded: &'loaded LoadedSchema, binding: TypedSchema<'loaded, T>) -> Self {
        Self { loaded, binding }
    }
}

impl<'loaded, T: Typol> TrustedBoundSchema<'loaded, T> {
    pub(crate) fn new(
        loaded: &'loaded LoadedSchema,
        binding: TrustedTypedSchema<'loaded, T>,
    ) -> Self {
        Self {
            inner: BoundSchema::new(loaded, binding.into_inner()),
        }
    }
}

impl<T: Typol> BoundSchema<'_, T> {
    pub fn schema(&self) -> &CompiledSchema {
        self.binding.schema()
    }

    pub fn read<'slot>(
        &self,
        input: ConfigInput<'_>,
        environment: &mut dyn Environment,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, T> {
        self.loaded.run_input(
            ConfigInputOperation::new(input, environment, TypolDecode { bound: self }),
            out,
        )
    }

    pub fn write<'slot>(
        &self,
        value: &T,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, String> {
        self.write_with_options(
            value,
            WriteOptions::default().with_conversion_limits(self.loaded.options().conversion()),
            out,
        )
    }

    pub fn write_with_options<'slot>(
        &self,
        value: &T,
        options: WriteOptions,
        mut out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, String> {
        let slot = self.loaded.error_slot(out.reborrow())?;
        let codec_options = CodecOptions::default()
            .with_validation(self.loaded.options().validation())
            .with_conversion(options.conversion_limits())
            .with_max_output_bytes(options.max_output_bytes());
        let mut codec_errors = ErrSlot::new();
        let result = codec_errors
            .try_call(|codec_out| self.binding.serialize(value, codec_options, codec_out));
        match result {
            Ok(text) => Ok(text),
            Err(caught) => Err(out.fail(slot.encoding(caught.take(), self.loaded.error_names()))),
        }
    }

    #[cfg(feature = "serde")]
    pub fn read_serde<'slot>(
        &self,
        input: ConfigInput<'_>,
        environment: &mut dyn Environment,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, T>
    where
        T: DeserializeOwned,
    {
        self.loaded.run_input(
            ConfigInputOperation::new(input, environment, SerdeDecode { bound: self }),
            out,
        )
    }

    #[cfg(feature = "serde")]
    pub fn write_serde<'slot>(
        &self,
        value: &T,
        out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, String>
    where
        T: Serialize,
    {
        self.write_serde_with_options(
            value,
            WriteOptions::default().with_conversion_limits(self.loaded.options().conversion()),
            out,
        )
    }

    #[cfg(feature = "serde")]
    pub fn write_serde_with_options<'slot>(
        &self,
        value: &T,
        options: WriteOptions,
        mut out: Out<'slot, '_, TypolError>,
    ) -> OResult<'slot, String>
    where
        T: Serialize,
    {
        let slot = self.loaded.error_slot(out.reborrow())?;
        let codec_options = CodecOptions::default()
            .with_validation(self.loaded.options().validation())
            .with_conversion(options.conversion_limits())
            .with_max_output_bytes(options.max_output_bytes());
        let mut codec_errors = ErrSlot::new();
        let result = codec_errors.try_call(|codec_out| {
            self.binding
                .serialize_serde(value, codec_options, codec_out)
        });
        match result {
            Ok(text) => Ok(text),
            Err(caught) => Err(out.fail(slot.encoding(caught.take(), self.loaded.error_names()))),
        }
    }
}
