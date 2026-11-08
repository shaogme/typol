mod arena;
mod builder;
mod builder_support;
mod copy;
mod errors;
mod views;

pub use builder::ConfigValueBuilder;
pub use builder_support::{NamedValue, ValuePayload};
pub use copy::ValueCopyLimits;
pub use errors::{AllocationError, ValueError, ValueErrorKind};
pub use views::{ConfigValue, NamedValueEntryView, ValueEntries, ValueEntryView, ValueView};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ValueId {
    pub(super) arena_id: usize,
    pub(super) index: usize,
}

/// The category of a configuration value source, excluding source text and environment values.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ValueSource {
    ConfigLiteral,
    SchemaDefault,
    EnvironmentVariable,
    ExplicitFallback,
    RustValue,
}

#[derive(Clone, Copy)]
pub enum ValueScalar<'a> {
    Signed(i128),
    Unsigned(u128),
    Float32(f32),
    Float64(f64),
    String(&'a str),
    Bool(bool),
}

pub struct EnvironmentReference<'a> {
    name: &'a str,
    fallback: Option<ValueScalar<'a>>,
}

impl<'a> EnvironmentReference<'a> {
    pub(super) const fn new(name: &'a str, fallback: Option<ValueScalar<'a>>) -> Self {
        Self { name, fallback }
    }

    pub const fn name(&self) -> &'a str {
        self.name
    }

    pub const fn fallback(&self) -> Option<&ValueScalar<'a>> {
        self.fallback.as_ref()
    }
}

#[derive(Clone, Copy)]
pub enum ValuePayloadView<'a> {
    Unit,
    Single(ValueView<'a>),
    Structure(ValueEntries<'a>),
}

pub enum ValueKind<'a> {
    Signed(i128),
    Unsigned(u128),
    Float32(f32),
    Float64(f64),
    String(&'a str),
    Bool(bool),
    Optional(Option<ValueView<'a>>),
    Array(ValueEntries<'a>),
    Map(ValueEntries<'a>),
    Structure(ValueEntries<'a>),
    Enumeration {
        variant: &'a str,
        payload: ValuePayloadView<'a>,
    },
}
