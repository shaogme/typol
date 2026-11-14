use core::fmt;

/// A diagnostic context description composed of static identifiers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticContextDescriptor {
    subsystem: &'static str,
    stage: &'static str,
    failure_point: &'static str,
}

impl DiagnosticContextDescriptor {
    pub const fn new(
        subsystem: &'static str,
        stage: &'static str,
        failure_point: &'static str,
    ) -> Self {
        Self {
            subsystem,
            stage,
            failure_point,
        }
    }

    pub const fn subsystem(self) -> &'static str {
        self.subsystem
    }

    pub const fn stage(self) -> &'static str {
        self.stage
    }

    pub const fn failure_point(self) -> &'static str {
        self.failure_point
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticObjectIdentifier {
    None,
    Id(u64),
    Index(usize),
    Range { start: usize, end: usize },
}

/// A compact context pointing to a static description with an optional bounded numeric object identifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DiagnosticContext {
    descriptor: &'static DiagnosticContextDescriptor,
    object: DiagnosticObjectIdentifier,
}

impl DiagnosticContext {
    pub const fn new(descriptor: &'static DiagnosticContextDescriptor) -> Self {
        Self {
            descriptor,
            object: DiagnosticObjectIdentifier::None,
        }
    }

    pub const fn with_object_id(mut self, object_id: u64) -> Self {
        self.object = DiagnosticObjectIdentifier::Id(object_id);
        self
    }

    /// Retains an exact `usize` index for structured error queries.
    pub const fn with_object_index(mut self, object_index: usize) -> Self {
        self.object = DiagnosticObjectIdentifier::Index(object_index);
        self
    }

    /// Retains both `usize` endpoints of a range exactly.
    pub const fn with_object_range(mut self, start: usize, end: usize) -> Self {
        self.object = DiagnosticObjectIdentifier::Range { start, end };
        self
    }

    pub const fn subsystem(self) -> &'static str {
        self.descriptor.subsystem()
    }

    pub const fn stage(self) -> &'static str {
        self.descriptor.stage()
    }

    pub const fn failure_point(self) -> &'static str {
        self.descriptor.failure_point()
    }

    pub const fn object_id(self) -> Option<u64> {
        match self.object {
            DiagnosticObjectIdentifier::Id(object_id) => Some(object_id),
            DiagnosticObjectIdentifier::None
            | DiagnosticObjectIdentifier::Index(_)
            | DiagnosticObjectIdentifier::Range { .. } => None,
        }
    }

    pub const fn object_identifier(self) -> DiagnosticObjectIdentifier {
        self.object
    }

    /// Returns the exactly retained `usize` object index.
    pub const fn object_index(self) -> Option<usize> {
        match self.object {
            DiagnosticObjectIdentifier::Index(index) => Some(index),
            _ => None,
        }
    }

    /// Returns exactly retained range endpoints.
    pub const fn object_range(self) -> Option<(usize, usize)> {
        match self.object {
            DiagnosticObjectIdentifier::Range { start, end } => Some((start, end)),
            _ => None,
        }
    }
}

/// A stable diagnostic code. Once assigned, its value and meaning do not change within the same major version.
#[derive(Clone, Copy, Eq, Hash, PartialEq)]
pub struct DiagnosticCode(u32);

impl DiagnosticCode {
    pub const RESOURCE_LIMIT_EXCEEDED: Self = Self(1);
    pub const ALLOCATION_FAILED: Self = Self(2);
    pub const LEXICAL_ERROR: Self = Self(3);
    pub const SCHEMA_SYNTAX_ERROR: Self = Self(4);
    pub const DIAGNOSTIC_LIMIT_REACHED: Self = Self(5);
    pub const REGEX_SYNTAX_ERROR: Self = Self(6);
    pub const SCHEMA_SEMANTIC_ERROR: Self = Self(7);
    pub const SCHEMA_RESOLVER_ERROR: Self = Self(8);
    pub const INTERNAL_ERROR: Self = Self(9);
    pub const CONFIG_SYNTAX_ERROR: Self = Self(10);
    pub const CONFIG_VALUE_ERROR: Self = Self(11);
    pub const CONFIG_VALIDATION_ERROR: Self = Self(12);
    pub const CONFIG_ENVIRONMENT_ERROR: Self = Self(13);
    pub const TYPE_BINDING_ERROR: Self = Self(14);
    pub const RUST_CONVERSION_ERROR: Self = Self(15);
    pub const CONFIG_ENCODING_ERROR: Self = Self(16);
    pub const SOURCE_INPUT_ERROR: Self = Self(17);
    pub const CAPACITY_EXCEEDED: Self = Self(18);
    pub const SCHEMA_CONTEXT_MISMATCH: Self = Self(19);
    pub const SCHEMA_SESSION_FINISHED: Self = Self(20);

    pub const fn new(value: u32) -> Option<Self> {
        if value == 0 { None } else { Some(Self(value)) }
    }

    pub const fn get(self) -> u32 {
        self.0
    }

    pub const fn name(self) -> &'static str {
        match self.0 {
            1 => "RESOURCE_LIMIT_EXCEEDED",
            2 => "ALLOCATION_FAILED",
            3 => "LEXICAL_ERROR",
            4 => "SCHEMA_SYNTAX_ERROR",
            5 => "DIAGNOSTIC_LIMIT_REACHED",
            6 => "REGEX_SYNTAX_ERROR",
            7 => "SCHEMA_SEMANTIC_ERROR",
            8 => "SCHEMA_RESOLVER_ERROR",
            9 => "INTERNAL_ERROR",
            10 => "CONFIG_SYNTAX_ERROR",
            11 => "CONFIG_VALUE_ERROR",
            12 => "CONFIG_VALIDATION_ERROR",
            13 => "CONFIG_ENVIRONMENT_ERROR",
            14 => "TYPE_BINDING_ERROR",
            15 => "RUST_CONVERSION_ERROR",
            16 => "CONFIG_ENCODING_ERROR",
            17 => "SOURCE_INPUT_ERROR",
            18 => "CAPACITY_EXCEEDED",
            19 => "SCHEMA_CONTEXT_MISMATCH",
            20 => "SCHEMA_SESSION_FINISHED",
            _ => "UNKNOWN",
        }
    }
}

impl fmt::Debug for DiagnosticCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("DiagnosticCode")
            .field(&self.0)
            .finish()
    }
}
