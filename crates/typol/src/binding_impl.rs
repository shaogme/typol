mod errors;
mod matcher;

#[cfg(all(test, feature = "std"))]
mod tests;

use alloc::string::String;
use errors::BindingPath;
pub use errors::{BindingErrors, BindingMismatch, BindingMismatchKind, BindingShape};

use crate::{
    binding_impl::matcher::{Matcher, MatcherBuffers, Pending, Task},
    codec::{SchemaCompleteValue, ValidatedValue},
    conversion::Conversion,
    descriptor::{BudgetedTypol, DescribeContext, DescriptorProvider, Typol},
    diagnostics::{CodecErrors, CodecStage, Failure},
    loading::BoundRead,
    options::{BindOptions, CodecOptions, ConversionLimits},
    session::SessionBudget,
};
use core::{any::TypeId, cmp::Ordering, marker::PhantomData, ops::Deref};
use opeo::{ErrSlot, OResult, Out, ResultOutExt};
use typol_config::{ConfigParseRequest, Environment};
use typol_core::{ChargedVec, DiagnosticContext, ExecutionBudget, diagnostic_context};
use typol_semantic::{CompiledSchema, FieldView, TypeView, VariantView};

#[cfg(feature = "serde")]
use crate::serde::Serde;
#[cfg(feature = "serde")]
use serde::{Serialize, de::DeserializeOwned};

#[cfg(all(test, feature = "std"))]
pub(crate) use tests::{BindingTestSupport, Root as BindingTestRoot};

static BINDING_INVARIANT: DiagnosticContext =
    diagnostic_context!("typol", "binding", "binding-plan-invariant");
static BINDING_PLAN_ENTRY_MISSING: DiagnosticContext =
    diagnostic_context!("typol", "binding", "conversion-type-binding-missing",);

pub(crate) struct FieldBinding<'schema> {
    pub(crate) rust_slot: usize,
    pub(crate) field: FieldView<'schema>,
}

pub(crate) struct FieldBindings<'schema> {
    fields: ChargedVec<FieldBinding<'schema>>,
    rust_slots: ChargedVec<usize>,
}

impl<'schema> FieldBindings<'schema> {
    pub(crate) fn empty() -> Self {
        Self {
            fields: ChargedVec::unmetered(),
            rust_slots: ChargedVec::unmetered(),
        }
    }

    pub(crate) fn new<'slot>(
        fields: ChargedVec<FieldBinding<'schema>>,
        rust_slot_count: usize,
        session: &SessionBudget,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        let mut rust_slots = session.budget_codec(
            CodecStage::Binding,
            |budget_out| session.workspace(budget_out),
            out.reborrow(),
        )?;
        session.budget_codec(
            CodecStage::Binding,
            |budget_out| rust_slots.try_reserve_exact(rust_slot_count, budget_out),
            out.reborrow(),
        )?;
        session.budget_codec(
            CodecStage::Binding,
            |budget_out| rust_slots.resize(rust_slot_count, usize::MAX, budget_out),
            out.reborrow(),
        )?;
        for (schema_slot, binding) in fields.iter().enumerate() {
            session.budget_codec(
                CodecStage::Binding,
                |budget_out| session.visit(budget_out),
                out.reborrow(),
            )?;
            let slot = rust_slots
                .get_mut(binding.rust_slot)
                .ok_or_else(internal_error)
                .store_err_into(out.reborrow())?;
            if *slot != usize::MAX {
                return Err(out.fail(internal_error()));
            }
            *slot = schema_slot;
        }
        Ok(Self { fields, rust_slots })
    }

    pub(crate) fn as_slice(&self) -> &[FieldBinding<'schema>] {
        &self.fields
    }

    pub(crate) fn by_rust_slot<'slot>(
        &self,
        slot: usize,
        session: &SessionBudget,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, FieldView<'schema>> {
        session.budget_codec(
            CodecStage::Conversion,
            |budget_out| session.visit(budget_out),
            out.reborrow(),
        )?;
        let field = self
            .rust_slots
            .get(slot)
            .and_then(|slot| self.fields.get(*slot))
            .map(|binding| binding.field)
            .ok_or_else(|| CodecErrors::emergency(CodecStage::Conversion, Failure::Conversion))
            .store_err_into(out.reborrow())?;
        Ok(field)
    }
}

pub(crate) struct VariantBinding<'schema> {
    pub(crate) rust_slot: usize,
    pub(crate) variant: VariantView<'schema>,
    pub(crate) fields: FieldBindings<'schema>,
}

pub(crate) struct TypeBinding<'schema> {
    pub(crate) identity: TypeId,
    pub(crate) ty: TypeView<'schema>,
    pub(crate) fields: FieldBindings<'schema>,
    pub(crate) variants: ChargedVec<VariantBinding<'schema>>,
}

pub(crate) struct DeserializeRequest<'request, 'environment, 'budget> {
    pub(crate) request: ConfigParseRequest<'request>,
    pub(crate) environment: &'environment mut dyn Environment,
    pub(crate) limits: ConversionLimits,
    pub(crate) budget: &'budget ExecutionBudget,
}

pub(crate) struct BindingPlan<'schema> {
    schema: &'schema CompiledSchema,
    types: ChargedVec<TypeBinding<'schema>>,
}

impl<'schema> BindingPlan<'schema> {
    pub(crate) fn schema(&self) -> &'schema CompiledSchema {
        self.schema
    }

    pub(crate) fn get<'slot>(
        &self,
        identity: TypeId,
        ty: TypeView<'schema>,
        session: &SessionBudget,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, &TypeBinding<'schema>> {
        let mut low = 0;
        let mut high = self.types.len();
        while low < high {
            session.budget_codec(
                CodecStage::Conversion,
                |budget_out| session.visit(budget_out),
                out.reborrow(),
            )?;
            let middle = low + (high - low) / 2;
            let binding = self
                .types
                .get(middle)
                .ok_or_else(internal_error)
                .store_err_into(out.reborrow())?;
            match binding
                .identity
                .cmp(&identity)
                .then_with(|| binding.ty.cmp(&ty))
            {
                Ordering::Less => low = middle + 1,
                Ordering::Greater => high = middle,
                Ordering::Equal => return Ok(binding),
            }
        }
        Err(out.fail(CodecErrors::emergency(
            CodecStage::Conversion,
            Failure::internal(BINDING_PLAN_ENTRY_MISSING),
        )))
    }
}

/// A compiled Schema binding for a built-in or derive-generated `Typol` type.
///
/// The binding budgets library-managed conversions and storage. It does not impose operating
/// system CPU, process-memory, or real-time limits; Serde implementations remain caller code.
/// `TypedSchema::bind` is available only when `T: BudgetedTypol`.
///
pub struct TypedSchema<'schema, T: Typol> {
    plan: BindingPlan<'schema>,
    _target: PhantomData<fn() -> T>,
}

/// A binding created through the explicit trusted extension path.
///
/// Its dereferenced `TypedSchema` operations retain the same library-managed budgets, but any
/// handwritten `Typol` callback may allocate, block, or compute outside those budgets. This type
/// records the API trust path; it is not a sandbox. Caller-provided Serde implementations are a
/// separate trust boundary as well.
///
pub struct TrustedTypedSchema<'schema, T: Typol> {
    inner: TypedSchema<'schema, T>,
}

impl<'schema, T: Typol> Deref for TrustedTypedSchema<'schema, T> {
    type Target = TypedSchema<'schema, T>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl<'schema, T: BudgetedTypol> TypedSchema<'schema, T> {
    /// Binds a type whose conversion implementation is built-in or derive-generated.
    pub fn bind<'slot>(
        schema: &'schema CompiledSchema,
        options: BindOptions,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        Self::bind_inner(schema, options, out)
    }
}

impl<'schema, T: Typol> TrustedTypedSchema<'schema, T> {
    pub(crate) fn into_inner(self) -> TypedSchema<'schema, T> {
        self.inner
    }
}

impl<'schema, T: Typol> TypedSchema<'schema, T> {
    fn bind_inner<'slot>(
        schema: &'schema CompiledSchema,
        options: BindOptions,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        let session = SessionBudget::codec_from_budget(
            CodecStage::Binding,
            |budget_out| SessionBudget::binding(options, budget_out),
            out.reborrow(),
        )?;
        let mut errors = ErrSlot::<CodecErrors>::new();
        let result = errors.try_call(|mut binding_out| {
            let mut descriptions =
                DescribeContext::new(options.limits(), &session, binding_out.reborrow())?;
            let buffers = MatcherBuffers {
                stack: descriptions.session().budget_codec(
                    CodecStage::Binding,
                    |budget_out| descriptions.session().workspace(budget_out),
                    binding_out.reborrow(),
                )?,
                active: descriptions.session().budget_codec(
                    CodecStage::Binding,
                    |budget_out| descriptions.session().workspace(budget_out),
                    binding_out.reborrow(),
                )?,
                types: descriptions.session().budget_codec(
                    CodecStage::Binding,
                    |budget_out| descriptions.session().workspace(budget_out),
                    binding_out.reborrow(),
                )?,
            };
            let mut matcher = Matcher::new(
                buffers,
                &descriptions,
                options.limits().max_diagnostics(),
                binding_out.reborrow(),
            )?;
            let root_path = BindingPath::root(&session, binding_out.reborrow())?;
            matcher.push(
                Task::Enter(Pending {
                    provider: DescriptorProvider::of::<T>(),
                    ty: schema.entry(),
                    depth: 0,
                    path: root_path,
                    field_context: false,
                }),
                &descriptions,
                binding_out.reborrow(),
            )?;
            while let Some(task) = matcher.pop_task() {
                if matcher.is_truncated() {
                    break;
                }
                descriptions.session().budget_codec(
                    CodecStage::Binding,
                    |budget_out| descriptions.session().visit(budget_out),
                    binding_out.reborrow(),
                )?;
                match task {
                    Task::Enter(pending) => {
                        matcher.enter(pending, &mut descriptions, binding_out.reborrow())?
                    }
                    Task::Exit(binding) => {
                        matcher.finish_binding(binding, &descriptions, binding_out.reborrow())?
                    }
                }
            }
            if matcher.has_mismatches() {
                let binding_errors = matcher.into_errors();
                let errors = CodecErrors::binding_mismatches(
                    binding_errors,
                    options.limits().max_diagnostics(),
                    descriptions.session(),
                    binding_out.reborrow(),
                )?;
                return Err(binding_out.fail(errors));
            }
            matcher.sort_types(&descriptions, binding_out.reborrow())?;
            Ok(Self {
                plan: BindingPlan {
                    schema,
                    types: matcher.into_types(),
                },
                _target: PhantomData,
            })
        });
        match result {
            Ok(schema) => Ok(schema),
            Err(caught) => {
                let errors = caught.take();
                Err(out.fail(errors))
            }
        }
    }

    pub fn schema(&self) -> &'schema CompiledSchema {
        self.plan.schema()
    }

    /// Converts and fully validates the actual value before emitting deterministic configuration text.
    ///
    /// `CodecOptions::with_pretty` selects multi-line output; compact text remains the default.
    /// Conversion diagnostics follow the bound Schema `@secret` boundary before storage.
    pub fn serialize<'slot>(
        &self,
        value: &T,
        options: CodecOptions,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, String> {
        let mut session = SessionBudget::codec_from_budget(
            CodecStage::Conversion,
            |budget_out| SessionBudget::codec(options, budget_out),
            out.reborrow(),
        )?;
        let value = SchemaCompleteValue::rust(Conversion::encode(
            self,
            value,
            &session,
            options.conversion(),
            out.reborrow(),
        )?);
        let mut validated = ValidatedValue::validate(
            self.schema(),
            value,
            options.validation(),
            &mut session,
            out.reborrow(),
        )?;
        validated.encode(options.pretty(), out.reborrow())
    }

    /// Parsing and final Rust construction share the request execution budget.
    /// Conversion diagnostics follow the bound Schema `@secret` boundary before storage.
    pub fn deserialize<'slot>(
        &self,
        request: ConfigParseRequest<'_>,
        environment: &mut dyn Environment,
        limits: ConversionLimits,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, T> {
        let session = SessionBudget::codec_from_budget(
            CodecStage::Parsing,
            |budget_out| SessionBudget::parsing(*request.options(), budget_out),
            out.reborrow(),
        )?;
        let value = ValidatedValue::parse_value(
            self.schema(),
            request,
            environment,
            &session,
            out.reborrow(),
        )?;
        Conversion::decode(
            self,
            value.as_config_value(),
            &session,
            limits,
            out.reborrow(),
        )
    }

    pub(crate) fn deserialize_with_value_and_budget<'slot>(
        &self,
        request: DeserializeRequest<'_, '_, '_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, BoundRead<T>> {
        let DeserializeRequest {
            request,
            environment,
            limits,
            budget,
        } = request;
        let session = SessionBudget::codec_from_budget(
            CodecStage::Parsing,
            |budget_out| SessionBudget::parsing_with_budget(*request.options(), budget, budget_out),
            out.reborrow(),
        )?;
        let value = ValidatedValue::parse_value(
            self.schema(),
            request,
            environment,
            &session,
            out.reborrow(),
        )?;
        let decoded = Conversion::decode(
            self,
            value.as_config_value(),
            &session,
            limits,
            out.reborrow(),
        )?;
        Ok(BoundRead::new(decoded, value))
    }

    #[cfg(feature = "serde")]
    pub(crate) fn deserialize_serde_with_budget<'slot>(
        &self,
        request: DeserializeRequest<'_, '_, '_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, T>
    where
        T: DeserializeOwned,
    {
        let DeserializeRequest {
            request,
            environment,
            limits,
            budget,
        } = request;
        let session = SessionBudget::codec_from_budget(
            CodecStage::Parsing,
            |budget_out| SessionBudget::parsing_with_budget(*request.options(), budget, budget_out),
            out.reborrow(),
        )?;
        let value = ValidatedValue::parse_value(
            self.schema(),
            request,
            environment,
            &session,
            out.reborrow(),
        )?;
        Serde::decode(
            self,
            value.as_config_value(),
            &session,
            limits,
            out.reborrow(),
        )
    }

    pub(crate) fn plan(&self) -> &BindingPlan<'schema> {
        &self.plan
    }

    /// Processes Serde events under the current binding’s Schema and emits configuration text after full validation.
    ///
    /// `CodecOptions::with_pretty` selects multi-line output; compact text remains the default.
    /// Serde and Typol effective names must match. Omitted fields are filled using Schema defaults or `none`; this does not guarantee reconstruction of the original Rust value.
    /// Actual output retains secret values and must not be used as a security log.
    /// Ordinary Serde trait calls without context do not guarantee Schema validation.
    #[cfg(feature = "serde")]
    pub fn serialize_serde<'slot>(
        &self,
        value: &T,
        options: CodecOptions,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, String>
    where
        T: Serialize,
    {
        let mut session = SessionBudget::codec_from_budget(
            CodecStage::Conversion,
            |budget_out| SessionBudget::codec(options, budget_out),
            out.reborrow(),
        )?;
        let value = SchemaCompleteValue::serde(Serde::encode(
            self,
            value,
            &session,
            options.conversion(),
            out.reborrow(),
        )?);
        let mut validated = ValidatedValue::validate(
            self.schema(),
            value,
            options.validation(),
            &mut session,
            out.reborrow(),
        )?;
        validated.encode(options.pretty(), out.reborrow())
    }

    /// Parses and validates the complete configuration before constructing the owned Serde target.
    ///
    /// Parsing, bridge event access, and library-controlled target text share the request budget; complete fields are provided, so Serde defaults do not replace Schema values.
    /// A caller-provided `Deserialize` implementation is a trust boundary: the library guarantees only that its input passed Schema validation. It cannot bound that implementation's internal work, blocking, or ordinary Rust allocations, and does not validate its arbitrary return value. `ConversionLimits` and the request budget do not constrain that user code.
    #[cfg(feature = "serde")]
    pub fn deserialize_serde<'slot>(
        &self,
        request: ConfigParseRequest<'_>,
        environment: &mut dyn Environment,
        limits: ConversionLimits,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, T>
    where
        T: DeserializeOwned,
    {
        let session = SessionBudget::codec_from_budget(
            CodecStage::Parsing,
            |budget_out| SessionBudget::parsing(*request.options(), budget_out),
            out.reborrow(),
        )?;
        let value = ValidatedValue::parse_value(
            self.schema(),
            request,
            environment,
            &session,
            out.reborrow(),
        )?;
        Serde::decode(
            self,
            value.as_config_value(),
            &session,
            limits,
            out.reborrow(),
        )
    }
}

impl<'schema, T: Typol> TypedSchema<'schema, T> {
    /// Binds a handwritten or otherwise explicitly trusted `Typol` implementation.
    ///
    /// The returned type records this path. Context-routed library work remains budgeted, but
    /// direct allocations, blocking, arbitrary computation, and returned Rust values are not
    /// constrained or revalidated by the library.
    pub fn bind_trusted<'slot>(
        schema: &'schema CompiledSchema,
        options: BindOptions,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, TrustedTypedSchema<'schema, T>> {
        Self::bind_inner(schema, options, out).map(|inner| TrustedTypedSchema { inner })
    }
}

fn internal_error() -> CodecErrors {
    CodecErrors::emergency(CodecStage::Binding, Failure::internal(BINDING_INVARIANT))
}
