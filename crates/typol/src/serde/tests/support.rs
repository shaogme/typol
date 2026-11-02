use crate::{
    BindingTestSupport,
    diagnostics::CodecErrors,
    options::{CodecOptions, ConversionLimits},
    serde::Model as OpeoModel,
    session::SessionBudget,
    test_support::{TestOpeo, TestOpeoOutcome},
};
use opeo::{OResult, Out};
use serde::{
    Deserialize, Deserializer, Serialize, Serializer,
    de::{EnumAccess, Error as DeError, MapAccess, VariantAccess, Visitor},
    ser::{Error as SerError, SerializeSeq, SerializeStruct, SerializeStructVariant},
};
use std::fmt;
use typol_core::{BudgetError, ConfigValue, DiagnosticCode, ExecutionLimits};
use typol_semantic::{CompiledSchema, TypeView};
use typol_test_support::{TestExpect, TestResult};

pub(crate) struct SerdeTestSupport;

impl SerdeTestSupport {
    pub(crate) fn session() -> TestResult<SessionBudget> {
        TestOpeo::value(|out| SessionBudget::codec(CodecOptions::default(), out))
    }

    pub(crate) fn new_session(
        execution: ExecutionLimits,
        max_text_bytes: usize,
        max_output_bytes: usize,
    ) -> TestResult<SessionBudget> {
        TestOpeo::value(|out| SessionBudget::new(execution, max_text_bytes, max_output_bytes, out))
    }

    pub(crate) fn compile(source: &str) -> TestResult<CompiledSchema> {
        BindingTestSupport::compile(source)
    }

    pub(crate) fn code(errors: CodecErrors) -> TestResult<u32> {
        BindingTestSupport::code(errors)
    }

    pub(crate) fn ty(schema: &CompiledSchema) -> TestResult<TypeView<'_>> {
        let field = TestOpeo::value(|out| schema.entry().field_named("value", out))?
            .test_expect("schema should contain the value field")?;
        TestOpeo::value(|out| field.ty(out))
    }
}

pub(crate) enum CallError<E> {
    Present(E),
    Missing,
}

pub(crate) enum CallOutcome<T, E> {
    Value(T),
    Error(E),
}

impl<T> CallOutcome<T, CodecErrors> {
    pub(crate) fn test_value(self) -> TestResult<T> {
        match self {
            Self::Value(value) => Ok(value),
            Self::Error(error) => Err(format!(
                "OPEO call failed: {error:?}; first diagnostic code: {:?}",
                error
                    .diagnostics()
                    .first()
                    .map(|diagnostic| diagnostic.code().get())
            )
            .into()),
        }
    }

    pub(crate) fn test_error(self) -> TestResult<CodecErrors> {
        match self {
            Self::Value(_) => Err("expected the OPEO call to fail, but it succeeded".into()),
            Self::Error(error) => Ok(error),
        }
    }

    pub(crate) fn is_ok(&self) -> bool {
        matches!(self, Self::Value(_))
    }

    pub(crate) fn is_err(&self) -> bool {
        matches!(self, Self::Error(_))
    }

    pub(crate) fn err(self) -> CallError<CodecErrors> {
        match self {
            Self::Value(_) => CallError::Missing,
            Self::Error(error) => CallError::Present(error),
        }
    }
}

impl<E: fmt::Debug> CallError<E> {
    pub(crate) fn test_value(self) -> TestResult<E> {
        match self {
            Self::Present(error) => Ok(error),
            Self::Missing => Err("expected the OPEO call to fail, but it succeeded".into()),
        }
    }
}

pub(crate) struct Model;

impl Model {
    pub(crate) fn encode<T: Serialize>(
        ty: TypeView<'_>,
        value: &T,
        session: &SessionBudget,
        limits: ConversionLimits,
    ) -> CallOutcome<ConfigValue, CodecErrors> {
        match TestOpeo::outcome(|out| OpeoModel::encode(ty, value, session, limits, out)) {
            TestOpeoOutcome::Value(value) => CallOutcome::Value(value),
            TestOpeoOutcome::Error(errors) => CallOutcome::Error(errors),
        }
    }

    pub(crate) fn decode<T: for<'de> Deserialize<'de>>(
        ty: TypeView<'_>,
        value: &ConfigValue,
        session: &SessionBudget,
        limits: ConversionLimits,
    ) -> CallOutcome<T, CodecErrors> {
        match TestOpeo::outcome(|out| OpeoModel::decode(ty, value, session, limits, out)) {
            TestOpeoOutcome::Value(value) => CallOutcome::Value(value),
            TestOpeoOutcome::Error(errors) => CallOutcome::Error(errors),
        }
    }
}

impl SerdeTestSupport {
    pub(crate) fn encode<T: Serialize>(
        schema: &CompiledSchema,
        value: &T,
    ) -> TestResult<CallOutcome<ConfigValue, CodecErrors>> {
        let ty = Self::ty(schema)?;
        let session = Self::session()?;
        Ok(
            match TestOpeo::outcome(|out| {
                OpeoModel::encode(ty, value, &session, ConversionLimits::default(), out)
            }) {
                TestOpeoOutcome::Value(value) => CallOutcome::Value(value),
                TestOpeoOutcome::Error(errors) => CallOutcome::Error(errors),
            },
        )
    }

    pub(crate) fn decode<T: for<'de> Deserialize<'de>>(
        schema: &CompiledSchema,
        value: &ConfigValue,
    ) -> TestResult<CallOutcome<T, CodecErrors>> {
        let ty = Self::ty(schema)?;
        let session = Self::session()?;
        Ok(
            match TestOpeo::outcome(|out| {
                OpeoModel::decode(ty, value, &session, ConversionLimits::default(), out)
            }) {
                TestOpeoOutcome::Value(value) => CallOutcome::Value(value),
                TestOpeoOutcome::Error(errors) => CallOutcome::Error(errors),
            },
        )
    }

    pub(crate) fn codec_value<T>(
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, CodecErrors>) -> OResult<'slot, T>,
    ) -> TestResult<T> {
        TestOpeo::value(operation)
    }

    pub(crate) fn codec_error<T>(
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, CodecErrors>) -> OResult<'slot, T>,
    ) -> TestResult<CodecErrors> {
        TestOpeo::error(operation)
    }

    pub(crate) fn allocation_session() -> TestResult<Option<SessionBudget>> {
        match TestOpeo::outcome(|out| SessionBudget::codec(CodecOptions::default(), out)) {
            TestOpeoOutcome::Value(session) => Ok(Some(session)),
            TestOpeoOutcome::Error(BudgetError::Allocation) => Ok(None),
            TestOpeoOutcome::Error(error) => {
                Err(format!("budget initialization failed: {error:?}").into())
            }
        }
    }

    pub(crate) fn allocation_codec_value<T>(
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, CodecErrors>) -> OResult<'slot, T>,
    ) -> TestResult<Option<T>> {
        match TestOpeo::outcome(operation) {
            TestOpeoOutcome::Value(value) => Ok(Some(value)),
            TestOpeoOutcome::Error(errors)
                if errors.diagnostics().last().is_some_and(|diagnostic| {
                    diagnostic.code() == DiagnosticCode::ALLOCATION_FAILED
                }) =>
            {
                Ok(None)
            }
            TestOpeoOutcome::Error(errors) => {
                Err(format!("operation returned a non-allocation error: {errors:?}").into())
            }
        }
    }
}

pub(crate) struct Identifier(String);

impl<'de> Deserialize<'de> for Identifier {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct NameVisitor;

        impl<'de> Visitor<'de> for NameVisitor {
            type Value = Identifier;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("known name")
            }

            fn visit_str<E: DeError>(self, value: &str) -> Result<Identifier, E> {
                Ok(Identifier(value.to_owned()))
            }
        }
        deserializer.deserialize_identifier(NameVisitor)
    }
}

pub(crate) struct FieldsEvent {
    name: &'static str,
    fields: &'static [&'static str],
}

impl Serialize for FieldsEvent {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut fields = serializer.serialize_struct(self.name, self.fields.len())?;
        for field in self.fields {
            fields.serialize_field(field, &7u8)?;
        }
        fields.end()
    }
}

impl FieldsEvent {
    pub(crate) fn new(name: &'static str, fields: &'static [&'static str]) -> Self {
        Self { name, fields }
    }
}

#[derive(Debug, PartialEq)]
pub(crate) enum Choice {
    Unit,
    Single(String),
    Structure { first: u8 },
}

impl Serialize for Choice {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Unit => serializer.serialize_unit_variant("Choice", 77, "Unit"),
            Self::Single(value) => {
                serializer.serialize_newtype_variant("Choice", 88, "Single", value)
            }
            Self::Structure { first } => {
                let mut fields =
                    serializer.serialize_struct_variant("Choice", 99, "Structure", 1)?;
                fields.serialize_field("first", first)?;
                fields.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Choice {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ChoiceVisitor;

        impl<'de> Visitor<'de> for ChoiceVisitor {
            type Value = Choice;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("Choice enum")
            }

            fn visit_enum<A: EnumAccess<'de>>(self, access: A) -> Result<Choice, A::Error> {
                let (name, variant) = access.variant::<Identifier>()?;
                match name.0.as_str() {
                    "Unit" => {
                        variant.unit_variant()?;
                        Ok(Choice::Unit)
                    }
                    "Single" => variant.newtype_variant().map(Choice::Single),
                    "Structure" => {
                        struct FieldsVisitor;

                        impl<'de> Visitor<'de> for FieldsVisitor {
                            type Value = Choice;

                            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                                formatter.write_str("anonymous payload")
                            }

                            fn visit_map<A: MapAccess<'de>>(
                                self,
                                mut access: A,
                            ) -> Result<Choice, A::Error> {
                                let key: Identifier = access.next_key()?.ok_or_else(|| {
                                    DeError::custom("variant payload is missing a field")
                                })?;
                                if key.0 != "first" {
                                    return Err(DeError::custom("secret unknown field"));
                                }
                                let first = access.next_value()?;
                                Ok(Choice::Structure { first })
                            }
                        }
                        variant.struct_variant(&["first"], FieldsVisitor)
                    }
                    _ => Err(DeError::custom("secret variant")),
                }
            }
        }
        deserializer.deserialize_enum("Choice", &["Unit", "Single", "Structure"], ChoiceVisitor)
    }
}

pub(crate) struct CustomError;

impl Serialize for CustomError {
    fn serialize<S: Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        Err(S::Error::custom("secret custom message"))
    }
}

impl<'de> Deserialize<'de> for CustomError {
    fn deserialize<D: Deserializer<'de>>(_: D) -> Result<Self, D::Error> {
        Err(DeError::custom("secret custom message"))
    }
}

pub(crate) struct NoConsume;

impl<'de> Deserialize<'de> for NoConsume {
    fn deserialize<D: Deserializer<'de>>(_: D) -> Result<Self, D::Error> {
        Ok(Self)
    }
}

pub(crate) struct Deep(usize);

impl Deep {
    pub(crate) fn new(depth: usize) -> Self {
        Self(depth)
    }
}

impl Serialize for Deep {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.0 == 0 {
            serializer.serialize_u8(1)
        } else {
            let mut sequence = serializer.serialize_seq(Some(usize::MAX))?;
            sequence.serialize_element(&Deep(self.0 - 1))?;
            sequence.end()
        }
    }
}
