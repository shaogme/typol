use crate::{
    diagnostics::{CodecErrors, CodecStage, Failure},
    options::ConversionLimits,
    session::{NodeBudget, SessionBudget, TargetCharge},
    value_walk::ValueDepth,
};
use alloc::string::String;
use core::{cell::Cell, error::Error as StdError, fmt};
use opeo::{ErrSlot, OResult, Out, ResultOutExt};
use serde::{de::Error as DeError, ser::Error as SerError};
use typol_core::{BudgetError, ConfigValue, DiagnosticContext, ValueError, diagnostic_context};
use typol_policy::ValueLimits;
use typol_semantic::{
    CopyError, CopyOptions, CopyRequest, SchemaAccessError, TypeKind, TypeView, ValueCopy,
    VariantView,
};

static SERDE_DATA_MODEL_UNSUPPORTED: DiagnosticContext =
    diagnostic_context!("typol", "serde", "serde-data-model-operation-unsupported");
static SERDE_USER_CUSTOM_REJECTION: DiagnosticContext =
    diagnostic_context!("typol", "serde", "serde-user-custom-rejection");
static SERDE_SCHEMA_SHAPE_MISMATCH: DiagnosticContext =
    diagnostic_context!("typol", "serde", "serde-schema-shape-mismatch");
static SERDE_COPY_INVARIANT: DiagnosticContext =
    diagnostic_context!("typol", "serde", "default-value-copy-invariant");

pub(crate) struct Error {
    failure: Failure,
}

impl Error {
    pub(crate) fn unsupported() -> Self {
        Self::failure(Failure::ConversionAt(SERDE_DATA_MODEL_UNSUPPORTED))
    }

    pub(crate) fn user_custom_rejection() -> Self {
        Self::failure(Failure::ConversionAt(SERDE_USER_CUSTOM_REJECTION))
    }

    pub(crate) fn shape_mismatch() -> Self {
        Self::failure(Failure::ConversionAt(SERDE_SCHEMA_SHAPE_MISMATCH))
    }

    pub(crate) fn failure(failure: Failure) -> Self {
        Self { failure }
    }

    pub(crate) fn internal(context: DiagnosticContext) -> Self {
        Self::failure(Failure::internal(context))
    }

    pub(crate) fn into_errors(self) -> CodecErrors {
        CodecErrors::emergency(CodecStage::Conversion, self.failure)
    }
}

impl From<BudgetError> for Error {
    fn from(error: BudgetError) -> Self {
        Self::failure(error.into())
    }
}

impl From<SchemaAccessError> for Error {
    fn from(error: SchemaAccessError) -> Self {
        Self::failure(Failure::schema_access(error))
    }
}

impl From<ValueError> for Error {
    fn from(error: ValueError) -> Self {
        Self::failure(error.into())
    }
}

impl From<CopyError> for Error {
    fn from(error: CopyError) -> Self {
        Self::failure(match error {
            CopyError::Resource => Failure::Resource,
            CopyError::Allocation => Failure::Allocation,
            CopyError::Capacity => Failure::Capacity,
            CopyError::Internal | CopyError::InternalAt(_) | CopyError::InvalidValue(_) => {
                Failure::internal(error.context().unwrap_or(SERDE_COPY_INVARIANT))
            }
        })
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SerdeError")
            .field("failure", &self.failure)
            .finish()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("serde conversion failed")
    }
}

impl StdError for Error {}

impl SerError for Error {
    fn custom<T: fmt::Display>(_: T) -> Self {
        Self::user_custom_rejection()
    }
}

impl DeError for Error {
    fn custom<T: fmt::Display>(_: T) -> Self {
        Self::user_custom_rejection()
    }
}

pub(crate) struct Fault {
    failure: Cell<Option<Failure>>,
}

pub(crate) struct State<'call> {
    session: &'call SessionBudget,
    fault: &'call Fault,
    nodes: NodeBudget,
    limits: ConversionLimits,
    target: TargetCharge,
}

pub(crate) struct Cursor<'a, 'schema, 'call> {
    state: &'a mut State<'call>,
    ty: TypeView<'schema>,
    depth: ValueDepth,
}

impl<'call> State<'call> {
    pub(crate) fn new<'slot>(
        session: &'call SessionBudget,
        limits: ConversionLimits,
        fault: &'call Fault,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, Self> {
        Ok(Self {
            session,
            fault,
            nodes: NodeBudget::new(limits.max_nodes(), limits.max_depth()),
            limits,
            target: session.budget_as(
                |budget_out| TargetCharge::new(session, budget_out),
                out.reborrow(),
            )?,
        })
    }

    pub(crate) fn root<'slot, 'a, 'schema>(
        &'a mut self,
        ty: TypeView<'schema>,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, Cursor<'a, 'schema, 'call>> {
        let depth = ValueDepth::root();
        self.session.budget_as(
            |budget_out| {
                self.nodes
                    .node(depth.level(), self.session.execution(), budget_out)
            },
            out.reborrow(),
        )?;
        Ok(Cursor {
            state: self,
            ty,
            depth,
        })
    }
}

impl<'a, 'schema, 'call> Cursor<'a, 'schema, 'call> {
    pub(crate) fn fault(&self) -> &'call Fault {
        self.state.fault
    }

    pub(crate) fn ty(&self) -> TypeView<'schema> {
        self.ty
    }

    pub(crate) fn session(&self) -> &'call SessionBudget {
        self.state.session
    }

    pub(crate) fn child<'slot, 'b>(
        &'b mut self,
        ty: TypeView<'schema>,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, Cursor<'b, 'schema, 'call>> {
        let depth = self
            .depth
            .child()
            .ok_or_else(|| Error::failure(Failure::Resource))
            .store_err_into(out.reborrow())?;
        self.session().budget_as(
            |budget_out| {
                self.state
                    .nodes
                    .node(depth.level(), self.session().execution(), budget_out)
            },
            out.reborrow(),
        )?;
        Ok(Cursor {
            state: self.state,
            ty,
            depth,
        })
    }

    pub(crate) fn reborrow(&mut self) -> Cursor<'_, 'schema, 'call> {
        Cursor {
            state: self.state,
            ty: self.ty,
            depth: self.depth,
        }
    }

    pub(crate) fn key<'slot>(&mut self, mut out: Out<'slot, '_, Error>) -> OResult<'slot, ()> {
        let depth = self
            .depth
            .child()
            .ok_or_else(|| Error::failure(Failure::Resource))
            .store_err_into(out.reborrow())?;
        self.session().budget_as(
            |budget_out| {
                self.state
                    .nodes
                    .node(depth.level(), self.session().execution(), budget_out)
            },
            out.reborrow(),
        )?;
        Ok(())
    }

    pub(crate) fn name<'slot>(
        &self,
        name: &str,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, ()> {
        let Some(expected) = self.session().schema_as(
            |schema_out| self.ty.nominal_name(schema_out),
            out.reborrow(),
        )?
        else {
            return Err(out.fail(Error::shape_mismatch()));
        };
        let ordering = self.session().budget_as(
            |budget_out| self.session().compare_text(expected, name, budget_out),
            out.reborrow(),
        )?;
        if ordering.is_eq() {
            Ok(())
        } else {
            Err(out.fail(Error::shape_mismatch()))
        }
    }

    pub(crate) fn variant<'slot>(
        &self,
        name: &str,
        variant: &str,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, VariantView<'schema>> {
        self.name(name, out.reborrow())?;
        let kind = self
            .session()
            .schema_as(|schema_out| self.ty.kind(schema_out), out.reborrow())?;
        if !matches!(kind, TypeKind::Enum { .. }) {
            return Err(out.fail(Error::shape_mismatch()));
        }
        let variant = self.session().budget_as(
            |budget_out| {
                self.ty
                    .variant_named_budgeted(variant, self.session().execution(), budget_out)
            },
            out.reborrow(),
        )?;
        let variant = variant
            .ok_or_else(Error::shape_mismatch)
            .store_err_into(out.reborrow())?;
        Ok(variant)
    }

    pub(crate) fn text<'slot>(
        &self,
        text: &str,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, ()> {
        self.session().budget_as(
            |budget_out| {
                self.session().execution().construct_text(
                    text.len(),
                    self.session().max_text_bytes(),
                    budget_out,
                )
            },
            out.reborrow(),
        )?;
        self.session().budget_as(
            |budget_out| self.session().execution().consume(text.len(), budget_out),
            out.reborrow(),
        )?;
        Ok(())
    }

    pub(crate) fn target_text<'slot>(
        &mut self,
        text: &str,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, String> {
        let session = self.state.session;
        let target_charge = &mut self.state.target;
        session.budget_as(
            |budget_out| session.copy_target_text(target_charge, text, budget_out),
            out.reborrow(),
        )
    }

    pub(crate) fn default_value<'slot>(
        &mut self,
        value: &ConfigValue,
        mut out: Out<'slot, '_, Error>,
    ) -> OResult<'slot, ConfigValue> {
        let depth = self
            .depth
            .child()
            .map(ValueDepth::level)
            .ok_or_else(|| Error::failure(Failure::Resource))
            .store_err_into(out.reborrow())?;
        let mut limits = ValueLimits::default();
        limits.set_max_value_nodes(
            self.state
                .limits
                .max_nodes()
                .saturating_sub(self.state.nodes.nodes()),
        );
        limits.set_max_nesting_depth(self.state.limits.max_depth());
        limits.set_max_string_bytes(self.session().max_text_bytes());
        let mut count = 0;
        let copied = self.session().copy_as(
            |copy_out| {
                ValueCopy::copy_value(
                    CopyRequest::new(
                        value,
                        depth,
                        &mut count,
                        &limits,
                        CopyOptions::new(
                            0,
                            Some(self.session().execution()),
                            self.session().max_text_bytes(),
                        ),
                    ),
                    copy_out,
                )
            },
            out.reborrow(),
        )?;
        for _ in 0..count {
            self.session().budget_as(
                |budget_out| {
                    self.state
                        .nodes
                        .node(depth, self.session().execution(), budget_out)
                },
                out.reborrow(),
            )?;
        }
        Ok(copied)
    }
}

impl Fault {
    pub(crate) fn new() -> Self {
        Self {
            failure: Cell::new(None),
        }
    }

    pub(crate) fn record<T>(&self, result: &Result<T, Error>) {
        if self.failure.get().is_none() {
            if let Err(error) = result {
                self.failure.set(Some(error.failure));
            }
        }
    }

    pub(crate) fn run<T>(
        &self,
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, Error>) -> OResult<'slot, T>,
    ) -> Result<T, Error> {
        if let Some(failure) = self.failure.get() {
            return Err(Error::failure(failure));
        }
        // Slots follow the callback stack; Cursor bounds recursive child dispatch.
        let mut errors = ErrSlot::new();
        let result = match errors.try_call(operation) {
            Ok(value) => Ok(value),
            Err(caught) => Err(caught.take()),
        };
        self.record(&result);
        result
    }

    pub(crate) fn unsupported<T>(&self) -> Result<T, Error> {
        self.run(|out| Err(out.fail(Error::unsupported())))
    }

    pub(crate) fn boundary<'slot, 'borrow, T>(
        result: Result<T, Error>,
        out: Out<'slot, 'borrow, Error>,
    ) -> OResult<'slot, T> {
        match result {
            Ok(value) => Ok(value),
            Err(error) => Err(out.fail(error)),
        }
    }

    pub(crate) fn finish<'slot, 'borrow, T>(
        &self,
        result: Result<T, Error>,
        out: Out<'slot, 'borrow, Error>,
    ) -> OResult<'slot, T> {
        match self.failure.get() {
            Some(failure) => Err(out.fail(Error::failure(failure))),
            None => match result {
                Ok(value) => Ok(value),
                Err(error) => Err(out.fail(error)),
            },
        }
    }

    pub(crate) fn complete<'slot, 'borrow, T>(
        &self,
        value: T,
        out: Out<'slot, 'borrow, Error>,
    ) -> OResult<'slot, T> {
        match self.failure.get() {
            Some(failure) => Err(out.fail(Error::failure(failure))),
            None => Ok(value),
        }
    }
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use super::Error;
    use serde::{de::Error as DeError, ser::Error as SerError};
    use typol_core::DiagnosticCode;
    use typol_test_support::{
        TestExpect, TestResult, test_verify as assert, test_verify_eq as assert_eq,
    };

    #[test]
    fn custom_serde_errors_keep_category_without_message_text() -> TestResult {
        let error = <Error as DeError>::custom("secret deserialization error");
        assert!(!format!("{error:?}{error}").contains("secret deserialization error"));
        let errors = error.into_errors();
        let diagnostic = errors
            .diagnostics()
            .first()
            .test_expect("serde errors should be converted to diagnostics")?;
        let context = errors
            .diagnostic_context(0)
            .test_expect("serde conversion errors should preserve static context")?;

        assert_eq!(diagnostic.code(), DiagnosticCode::RUST_CONVERSION_ERROR);
        assert_eq!(context.subsystem(), "typol");
        assert_eq!(context.stage(), "serde");
        assert_eq!(context.failure_point(), "serde-user-custom-rejection");
        assert!(!format!("{errors:?}{errors}").contains("secret deserialization error"));

        let error = <Error as SerError>::custom("secret serialization error");
        assert!(!format!("{error:?}{error}").contains("secret serialization error"));
        let errors = error.into_errors();
        let context = errors
            .diagnostic_context(0)
            .test_expect("custom serialization rejections should preserve static context")?;
        assert_eq!(context.failure_point(), "serde-user-custom-rejection");
        assert!(!format!("{errors:?}{errors}").contains("secret serialization error"));

        let categories = [
            Error::unsupported(),
            Error::user_custom_rejection(),
            Error::shape_mismatch(),
        ];
        let expected = [
            "serde-data-model-operation-unsupported",
            "serde-user-custom-rejection",
            "serde-schema-shape-mismatch",
        ];
        for (error, expected) in categories.into_iter().zip(expected) {
            let context = error
                .into_errors()
                .diagnostic_context(0)
                .test_expect("serde rejections should carry classification context")?;
            assert_eq!(context.failure_point(), expected);
        }
        Ok(())
    }
}
