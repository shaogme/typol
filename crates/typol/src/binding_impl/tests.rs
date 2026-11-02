mod diagnostics;

use crate::{
    binding_impl::{BindingMismatchKind, BindingShape, TrustedTypedSchema, TypedSchema},
    conversion::{ConversionContext, ConversionValue},
    descriptor::{
        BudgetedTypol, DescribeContext, DescriptorNode, DescriptorProvider, MapContainer,
        TypeDescriptor, TypeShape, Typol, VariantShape,
    },
    diagnostics::{CodecErrors, CodecStage},
    options::{BindOptions, BindingLimits, CodecOptions},
    session::SessionBudget,
    test_support::TestOpeo,
};
use opeo::{OResult, Out};
use std::{any::TypeId, cmp::Ordering};
use typol_core::{ExecutionLimits, ValueId};
use typol_model::BuiltinType;
use typol_schema::{
    ResolverError, SchemaCompileOptions, SchemaCompiler, SchemaResolveRequest, SchemaResolver,
    SchemaSource,
};
use typol_semantic::{CompiledSchema, SchemaAccessError};
use typol_test_support::{
    TestExpect, TestExtract, TestResult, test_verify as assert, test_verify_eq as assert_eq,
    test_verify_ne as assert_ne,
};

macro_rules! rejected_typol_impl {
    ($ty:ty, |$context:ident, $out:ident| $describe:block) => {
        impl Typol for $ty {
            fn describe<'slot, 'borrow>(
                $context: &mut DescribeContext<'_>,
                mut $out: Out<'slot, 'borrow, CodecErrors>,
            ) -> OResult<'slot, TypeDescriptor> $describe

            fn to_config_value<'slot, 'borrow>(
                &self,
                _: &mut ConversionContext<'_>,
                out: Out<'slot, 'borrow, CodecErrors>,
            ) -> OResult<'slot, ValueId> {
                ConversionContext::reject(out)
            }

            fn from_config_value<'slot, 'borrow>(
                _value: ConversionValue<'_>,
                _: &mut ConversionContext<'_>,
                out: Out<'slot, 'borrow, CodecErrors>,
            ) -> OResult<'slot, Self> {
                ConversionContext::reject(out)
            }
        }
    };
}

pub(crate) struct BindingTestSupport;

fn present<T>(value: Option<T>) -> TestResult<T> {
    value.test_expect("schema should contain the expected member")
}

fn access<T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, SchemaAccessError>) -> OResult<'slot, T>,
) -> TestResult<T> {
    TestOpeo::value(operation)
}

fn binding_session() -> TestResult<SessionBudget> {
    TestOpeo::value(|out| SessionBudget::binding(BindOptions::default(), out))
}

fn bind<'schema, T: Typol>(
    schema: &'schema CompiledSchema,
    options: BindOptions,
) -> TestResult<TrustedTypedSchema<'schema, T>> {
    TestOpeo::value(|out| TypedSchema::<T>::bind_trusted(schema, options, out))
}

fn bind_error<T: Typol>(schema: &CompiledSchema, options: BindOptions) -> TestResult<CodecErrors> {
    TestOpeo::error(|out| TypedSchema::<T>::bind_trusted(schema, options, out))
}

fn bind_code<T: Typol>(schema: &CompiledSchema, options: BindOptions) -> TestResult<u32> {
    code(bind_error::<T>(schema, options)?)
}

fn codec_value<T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, CodecErrors>) -> OResult<'slot, T>,
) -> TestResult<T> {
    TestOpeo::value(operation)
}

fn codec_error<T>(
    operation: impl for<'slot> FnOnce(Out<'slot, 'slot, CodecErrors>) -> OResult<'slot, T>,
) -> TestResult<CodecErrors> {
    TestOpeo::error(operation)
}

fn codec_session() -> TestResult<SessionBudget> {
    TestOpeo::value(|out| SessionBudget::codec(CodecOptions::default(), out))
}

struct NoImports;

impl SchemaResolver for NoImports {
    fn resolve<'slot, 'borrow, 'src>(
        &'src mut self,
        _request: SchemaResolveRequest<'_>,
        out: Out<'slot, 'borrow, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'src>> {
        Err(out.fail(ResolverError::NotFound))
    }
}

fn compile(source: &str) -> TestResult<CompiledSchema> {
    let compiler =
        TestOpeo::value(|out| SchemaCompiler::new(SchemaCompileOptions::default(), out))?;
    let mut resolver = NoImports;
    TestOpeo::value(|out| {
        compiler.compile(
            SchemaSource::from_str(b"binding", source),
            &mut resolver,
            None,
            out,
        )
    })
}

fn code(errors: CodecErrors) -> TestResult<u32> {
    Ok(errors
        .diagnostics()
        .last()
        .test_expect("error diagnostic list should include the final diagnostic")?
        .code()
        .get())
}

fn structure<'slot, T: 'static>(
    context: &mut DescribeContext<'_>,
    name: &str,
    fields: &[(&str, DescriptorProvider)],
    mut out: Out<'slot, '_, CodecErrors>,
) -> OResult<'slot, TypeDescriptor> {
    let mut members = context.session().budget_codec(
        CodecStage::Binding,
        |budget_out| context.session().workspace(budget_out),
        out.reborrow(),
    )?;
    context.session().budget_codec(
        CodecStage::Binding,
        |budget_out| members.try_reserve_exact(fields.len(), budget_out),
        out.reborrow(),
    )?;
    for (name, value) in fields {
        let field_name = context.name(name, out.reborrow())?;
        let field = context.insert(
            DescriptorNode::Field {
                name: field_name,
                value: *value,
            },
            context.depth(),
            out.reborrow(),
        )?;
        context.session().budget_codec(
            CodecStage::Binding,
            |budget_out| members.try_push(field, budget_out),
            out.reborrow(),
        )?;
    }
    let type_name = context.name(name, out.reborrow())?;
    context.insert(
        DescriptorNode::Type {
            identity: TypeId::of::<T>(),
            name: Some(type_name),
            shape: TypeShape::Structure(members),
        },
        context.depth(),
        out.reborrow(),
    )
}

#[derive(Debug, PartialEq)]
pub(crate) struct Root<T> {
    value: T,
}

impl<T> Root<T> {
    pub(crate) fn new(value: T) -> Self {
        Self { value }
    }

    pub(crate) fn into_value(self) -> T {
        self.value
    }
}

impl<T: Typol> Typol for Root<T> {
    fn describe<'slot, 'borrow>(
        context: &mut DescribeContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        structure::<Self>(
            context,
            "Root",
            &[("value", DescriptorProvider::of::<T>())],
            out,
        )
    }

    fn to_config_value<'slot>(
        &self,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        context.start::<Self>(out.reborrow())?;
        context.structure(
            |_, child, child_out| child.encode_here(&self.value, child_out),
            out.reborrow(),
        )
    }

    fn from_config_value<'slot>(
        value: ConversionValue<'_>,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        context.start::<Self>(out.reborrow())?;
        let fields = context.read_structure(value, out.reborrow())?;
        let value = context.read_field(fields, 0, out.reborrow())?;
        Ok(Self { value })
    }
}

impl<T: BudgetedTypol> BudgetedTypol for Root<T> {}

#[test]
fn all_supported_shapes_bind_exactly() -> TestResult {
    macro_rules! scalar {
        ($($rust:ty => $dsl:literal),* $(,)?) => { $(
            let schema = compile(concat!("entry struct Root { value: ", $dsl, " }"))?;
            bind::<Root<$rust>>(&schema, BindOptions::default())?;
        )* };
    }
    scalar!(bool => "bool", i8 => "i8", i16 => "i16", i32 => "i32", i64 => "i64", i128 => "i128", u8 => "u8", u16 => "u16", u32 => "u32", u64 => "u64", u128 => "u128", f32 => "f32", f64 => "f64", String => "string", Option<u8> => "option<u8>", Vec<u8> => "[u8]", [u8; 0] => "[u8; 0]", [u8; 2] => "[u8; 2]");
    Ok(())
}

#[test]
fn width_option_and_array_mismatches_are_binding_errors() -> TestResult {
    for source in [
        "entry struct Root { value: i8 }",
        "entry struct Root { value: u16 }",
        "entry struct Root { value: option<u8> }",
        "entry struct Root { renamed: u8 }",
    ] {
        assert_eq!(
            bind_code::<Root<u8>>(&compile(source)?, BindOptions::default())?,
            14
        );
    }
    let schema = compile("entry struct Root { value: [u8; 3] }")?;
    assert_eq!(
        bind_code::<Root<[u8; 2]>>(&schema, BindOptions::default())?,
        14
    );
    assert_eq!(
        bind_code::<Root<Vec<u8>>>(&schema, BindOptions::default())?,
        14
    );
    let schema = compile("entry struct Root { value: [u8] }")?;
    assert_eq!(
        bind_code::<Root<[u8; 2]>>(&schema, BindOptions::default())?,
        14
    );
    Ok(())
}

pub(crate) struct Bad<const MODE: usize>;

impl<const MODE: usize> Typol for Bad<MODE> {
    fn describe<'slot>(
        context: &mut DescribeContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        match MODE {
            0 => structure::<Self>(context, "Root", &[], out.reborrow()),
            1 => structure::<Self>(
                context,
                "Root",
                &[
                    ("value", DescriptorProvider::of::<u8>()),
                    ("extra", DescriptorProvider::of::<u8>()),
                ],
                out.reborrow(),
            ),
            2 => structure::<Self>(
                context,
                "Root",
                &[
                    ("value", DescriptorProvider::of::<u8>()),
                    ("value", DescriptorProvider::of::<u8>()),
                ],
                out.reborrow(),
            ),
            3 => structure::<Self>(
                context,
                "Other",
                &[("value", DescriptorProvider::of::<u8>())],
                out.reborrow(),
            ),
            _ => {
                let session = SessionBudget::codec_from_budget(
                    CodecStage::Binding,
                    |budget_out| SessionBudget::binding(BindOptions::default(), budget_out),
                    out.reborrow(),
                )?;
                let mut foreign =
                    DescribeContext::new(BindingLimits::default(), &session, out.reborrow())?;
                u8::describe(&mut foreign, out.reborrow())
            }
        }
    }

    fn to_config_value<'slot, 'borrow>(
        &self,
        _: &mut ConversionContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        ConversionContext::reject(out)
    }

    fn from_config_value<'slot, 'borrow>(
        _value: ConversionValue<'_>,
        _: &mut ConversionContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, Self> {
        ConversionContext::reject(out)
    }
}

#[test]
fn missing_extra_duplicate_nominal_and_foreign_descriptors_are_rejected() -> TestResult {
    let schema = compile("entry struct Root { value: u8 }")?;
    assert_eq!(bind_code::<Bad<0>>(&schema, BindOptions::default())?, 14);
    assert_eq!(bind_code::<Bad<1>>(&schema, BindOptions::default())?, 14);
    assert_eq!(bind_code::<Bad<3>>(&schema, BindOptions::default())?, 14);
    assert_eq!(bind_code::<Bad<4>>(&schema, BindOptions::default())?, 9);
    let schema = compile("entry struct Root { value: u8, other: u8 }")?;
    assert_eq!(bind_code::<Bad<2>>(&schema, BindOptions::default())?, 14);
    Ok(())
}

#[test]
fn binding_mismatches_preserve_stable_reasons_paths_and_shapes() -> TestResult {
    let schema = compile("entry struct Root { value: u8 }")?;
    let errors = bind_error::<Bad<3>>(&schema, BindOptions::default())?;
    let error = errors
        .binding_errors()
        .and_then(|errors| errors.first())
        .test_expect("binding errors should include a structured incompatibility reason")?;
    assert_eq!(error.kind(), BindingMismatchKind::RootTypeName);
    assert_eq!(error.schema_path(), "$.Root");
    assert_eq!(error.rust_path(), "$.Other");

    let errors = bind_error::<Bad<0>>(&schema, BindOptions::default())?;
    let error = errors
        .binding_errors()
        .and_then(|errors| errors.first())
        .test_expect("binding errors should include a structured incompatibility reason")?;
    assert_eq!(error.kind(), BindingMismatchKind::MissingField);
    assert_eq!(error.schema_path(), "$.Root.value");
    assert_eq!(error.rust_path(), "$.Root");

    let errors = bind_error::<Bad<1>>(&schema, BindOptions::default())?;
    let error = errors
        .binding_errors()
        .and_then(|errors| errors.first())
        .test_expect("binding errors should include a structured incompatibility reason")?;
    assert_eq!(error.kind(), BindingMismatchKind::ExtraField);
    assert_eq!(error.schema_path(), "$.Root");
    assert_eq!(error.rust_path(), "$.Root.extra");

    let errors = bind_error::<Root<i8>>(&schema, BindOptions::default())?;
    let error = errors
        .binding_errors()
        .and_then(|errors| errors.first())
        .test_expect("binding errors should include a structured incompatibility reason")?;
    assert_eq!(error.kind(), BindingMismatchKind::FieldShape);
    assert_eq!(error.schema_path(), "$.Root.value");
    assert_eq!(error.expected(), BindingShape::Builtin(BuiltinType::U8));
    assert_eq!(error.actual(), BindingShape::Builtin(BuiltinType::I8));

    let schema = compile("entry struct Root { value: [u8; 3] }")?;
    let errors = bind_error::<Root<[u8; 2]>>(&schema, BindOptions::default())?;
    let error = errors
        .binding_errors()
        .and_then(|errors| errors.first())
        .test_expect("binding errors should include a structured incompatibility reason")?;
    assert_eq!(error.kind(), BindingMismatchKind::ArrayLength);
    assert_eq!(error.expected(), BindingShape::Array { length: Some(3) });
    assert_eq!(error.actual(), BindingShape::Array { length: Some(2) });
    Ok(())
}

#[derive(Debug, PartialEq)]
pub(crate) struct Empty;

impl Typol for Empty {
    fn describe<'slot, 'borrow>(
        context: &mut DescribeContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        structure::<Self>(context, "Empty", &[], out)
    }

    fn to_config_value<'slot>(
        &self,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        context.start::<Self>(out.reborrow())?;
        context.structure(
            |_, _, child_out| ConversionContext::reject(child_out),
            out.reborrow(),
        )
    }

    fn from_config_value<'slot>(
        value: ConversionValue<'_>,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        context.start::<Self>(out.reborrow())?;
        context.read_structure(value, out.reborrow())?;
        Ok(Self)
    }
}

#[test]
fn aliases_use_bottom_nominal_names_and_keep_use_site_metadata() -> TestResult {
    let schema = compile(
        "struct Empty {} type Alias = Empty; type Chain = Alias; entry struct Root { value: Chain }",
    )?;
    let binding = bind::<Root<Empty>>(&schema, BindOptions::default())?;
    let fields = access(|out| schema.entry().fields(out))?;
    let field = present(fields.get(0))?;
    let ty = access(|out| field.ty(out))?;
    assert_eq!(access(|out| ty.name(out))?, Some("Empty"));
    assert_eq!(access(|out| ty.nominal_name(out))?, Some("Empty"));
    let session = binding_session()?;
    assert_eq!(
        codec_value(|out| binding
            .plan()
            .get(TypeId::of::<Empty>(), ty, &session, out,))?
        .ty,
        ty
    );
    let other = compile("struct Empty {} type Alias = Empty; entry struct Root { value: Alias }")?;
    let other_fields = access(|out| other.entry().fields(out))?;
    let other_field = present(other_fields.get(0))?;
    let other_type = access(|out| other_field.ty(out))?;
    assert_eq!(
        code(codec_error(|out| binding.plan().get(
            TypeId::of::<Empty>(),
            other_type,
            &session,
            out,
        ))?)?,
        9
    );
    Ok(())
}

struct Recursive;

rejected_typol_impl!(Recursive, |context, out| {
    structure::<Self>(
        context,
        "Root",
        &[("value", DescriptorProvider::of::<Self>())],
        out.reborrow(),
    )
});

#[test]
fn active_type_cycles_and_resource_limits_are_rejected_before_child_resolution() -> TestResult {
    let schema = compile("entry struct Root { value: u8 }")?;
    assert_eq!(bind_code::<Recursive>(&schema, BindOptions::default())?, 14);
    for limits in [
        BindingLimits::new(0, 128, 100, 262_144).test_value()?,
        BindingLimits::new(100, 0, 100, 262_144).test_value()?,
        BindingLimits::new(100, 128, 100, 3).test_value()?,
    ] {
        let options = BindOptions::new(limits, ExecutionLimits::new(16_777_216, 67_108_864));
        assert_eq!(bind_code::<Root<u8>>(&schema, options)?, 1);
    }
    Ok(())
}

fn enum_descriptor<'slot, T: 'static>(
    context: &mut DescribeContext<'_>,
    name: &str,
    shapes: Vec<(&str, VariantShape)>,
    mut out: Out<'slot, '_, CodecErrors>,
) -> OResult<'slot, TypeDescriptor> {
    let mut variants = context.session().budget_codec(
        CodecStage::Binding,
        |budget_out| context.session().workspace(budget_out),
        out.reborrow(),
    )?;
    context.session().budget_codec(
        CodecStage::Binding,
        |budget_out| variants.try_reserve_exact(shapes.len(), budget_out),
        out.reborrow(),
    )?;
    for (name, shape) in shapes {
        let variant_name = context.name(name, out.reborrow())?;
        let variant = context.insert(
            DescriptorNode::Variant {
                name: variant_name,
                shape,
            },
            context.depth(),
            out.reborrow(),
        )?;
        context.session().budget_codec(
            CodecStage::Binding,
            |budget_out| variants.try_push(variant, budget_out),
            out.reborrow(),
        )?;
    }
    let type_name = context.name(name, out.reborrow())?;
    context.insert(
        DescriptorNode::Type {
            identity: TypeId::of::<T>(),
            name: Some(type_name),
            shape: TypeShape::Enumeration(variants),
        },
        context.depth(),
        out.reborrow(),
    )
}

struct MissingVariants;

rejected_typol_impl!(MissingVariants, |context, out| {
    enum_descriptor::<Self>(
        context,
        "Choice",
        vec![("One", VariantShape::Unit)],
        out.reborrow(),
    )
});

struct ExtraVariants;

rejected_typol_impl!(ExtraVariants, |context, out| {
    enum_descriptor::<Self>(
        context,
        "Choice",
        vec![("One", VariantShape::Unit), ("Extra", VariantShape::Unit)],
        out.reborrow(),
    )
});

struct PayloadMismatch;

rejected_typol_impl!(PayloadMismatch, |context, out| {
    enum_descriptor::<Self>(
        context,
        "Choice",
        vec![("One", VariantShape::Unit)],
        out.reborrow(),
    )
});

struct NestedChild;

rejected_typol_impl!(NestedChild, |context, out| {
    structure::<Self>(
        context,
        "Child",
        &[("value", DescriptorProvider::of::<u8>())],
        out.reborrow(),
    )
});

struct NestedRoot;

rejected_typol_impl!(NestedRoot, |context, out| {
    structure::<Self>(
        context,
        "Root",
        &[("child", DescriptorProvider::of::<NestedChild>())],
        out.reborrow(),
    )
});

struct MapMismatch;

rejected_typol_impl!(MapMismatch, |context, out| {
    context.insert(
        DescriptorNode::Type {
            identity: TypeId::of::<Self>(),
            name: None,
            shape: TypeShape::Map {
                container: MapContainer::HashMap,
                value: DescriptorProvider::of::<u8>(),
            },
        },
        context.depth(),
        out.reborrow(),
    )
});

pub(crate) struct DuplicateVariants;

rejected_typol_impl!(DuplicateVariants, |context, out| {
    enum_descriptor::<Self>(
        context,
        "Choice",
        vec![("One", VariantShape::Unit), ("One", VariantShape::Unit)],
        out.reborrow(),
    )
});

pub(crate) struct MultipleFieldMismatches;

rejected_typol_impl!(MultipleFieldMismatches, |context, out| {
    structure::<Self>(
        context,
        "Root",
        &[
            ("alpha", DescriptorProvider::of::<u16>()),
            ("extra", DescriptorProvider::of::<bool>()),
        ],
        out.reborrow(),
    )
});

#[test]
fn enum_map_and_nested_binding_errors_keep_specific_paths() -> TestResult {
    let schema = compile("enum Choice { One, Two } entry struct Root { value: Choice }")?;
    let errors = bind_error::<Root<MissingVariants>>(&schema, BindOptions::default())?;
    let mismatch = errors
        .binding_errors()
        .and_then(|errors| errors.first())
        .test_expect("missing variants should preserve the structured reason")?;
    assert_eq!(mismatch.kind(), BindingMismatchKind::MissingVariant);
    assert_eq!(mismatch.schema_path(), "$.Root.value.Choice.Two");
    assert_eq!(mismatch.rust_path(), "$.Root.value.Choice");

    let schema = compile("enum Choice { One } entry struct Root { value: Choice }")?;
    let errors = bind_error::<Root<ExtraVariants>>(&schema, BindOptions::default())?;
    let mismatch = errors
        .binding_errors()
        .and_then(|errors| errors.first())
        .test_expect("extra variants should preserve the structured reason")?;
    assert_eq!(mismatch.kind(), BindingMismatchKind::ExtraVariant);
    assert_eq!(mismatch.schema_path(), "$.Root.value.Choice");
    assert_eq!(mismatch.rust_path(), "$.Root.value.Choice.Extra");

    let schema = compile("enum Choice { One(u8) } entry struct Root { value: Choice }")?;
    let errors = bind_error::<Root<PayloadMismatch>>(&schema, BindOptions::default())?;
    let mismatch = errors
        .binding_errors()
        .and_then(|errors| errors.first())
        .test_expect("payload shape mismatches should preserve the structured reason")?;
    assert_eq!(mismatch.kind(), BindingMismatchKind::VariantPayload);
    assert_eq!(mismatch.schema_path(), "$.Root.value.Choice.One");
    assert_eq!(mismatch.expected(), BindingShape::SinglePayload);
    assert_eq!(mismatch.actual(), BindingShape::UnitPayload);

    let schema = compile("entry struct Root { value: u8 }")?;
    let errors = bind_error::<Root<MapMismatch>>(&schema, BindOptions::default())?;
    let mismatch = errors
        .binding_errors()
        .and_then(|errors| errors.first())
        .test_expect("map container mismatches should preserve the structured reason")?;
    assert_eq!(mismatch.kind(), BindingMismatchKind::MapContainer);
    assert_eq!(
        mismatch.actual(),
        BindingShape::Map {
            container: Some(MapContainer::HashMap)
        }
    );

    let schema = compile("struct Child { value: string } entry struct Root { child: Child }")?;
    let errors = bind_error::<NestedRoot>(&schema, BindOptions::default())?;
    let mismatch = errors
        .binding_errors()
        .and_then(|errors| errors.first())
        .test_expect("nested field failures should preserve the full path")?;
    assert_eq!(mismatch.kind(), BindingMismatchKind::FieldShape);
    assert_eq!(mismatch.schema_path(), "$.Root.child.Child.value");
    let diagnostic = errors
        .diagnostics()
        .first()
        .test_expect("binding diagnostics should preserve a readable reason")?;
    assert!(diagnostic.message().contains("field shape mismatch"));
    assert!(diagnostic.message().contains("$.Root.child.Child.value"));
    Ok(())
}

#[test]
fn equal_short_names_in_different_schema_modules_preserve_owner_views() -> TestResult {
    let schema = compile(
        "mod left { struct Empty {} } mod right { struct Empty {} } entry struct Root { first: left::Empty, second: right::Empty }",
    )?;
    let fields = access(|out| schema.entry().fields(out))?;
    let left_field = present(fields.get(0))?;
    let right_field = present(fields.get(1))?;
    let left = access(|out| left_field.ty(out))?;
    let right = access(|out| right_field.ty(out))?;
    assert_eq!(
        access(|out| left.nominal_name(out))?,
        access(|out| right.nominal_name(out))?
    );
    assert!(!access(|out| left.same_type(right, out))?);
    assert_ne!(left.cmp(&right), Ordering::Equal);
    assert_eq!(left.cmp(&right), right.cmp(&left).reverse());
    assert_eq!(left.cmp(&left), Ordering::Equal);
    struct Two;
    rejected_typol_impl!(Two, |context, out| {
        structure::<Self>(
            context,
            "Root",
            &[
                ("first", DescriptorProvider::of::<Empty>()),
                ("second", DescriptorProvider::of::<Empty>()),
            ],
            out.reborrow(),
        )
    });
    let binding = bind::<Two>(&schema, BindOptions::default())?;
    let session = binding_session()?;
    assert_eq!(
        codec_value(|out| binding
            .plan()
            .get(TypeId::of::<Empty>(), left, &session, out))?
        .ty,
        left
    );
    assert_eq!(
        codec_value(|out| binding
            .plan()
            .get(TypeId::of::<Empty>(), right, &session, out))?
        .ty,
        right
    );
    Ok(())
}

struct Indirect;

rejected_typol_impl!(Indirect, |context, out| {
    structure::<Self>(
        context,
        "Root",
        &[("value", DescriptorProvider::of::<Vec<Self>>())],
        out.reborrow(),
    )
});

#[test]
fn indirect_container_cycle_is_detected_by_actual_rust_identity() -> TestResult {
    let schema = compile("entry struct Root { value: [u8] }")?;
    assert_eq!(bind_code::<Indirect>(&schema, BindOptions::default())?, 14);
    Ok(())
}

#[test]
fn binding_retains_alias_and_field_rules_without_validating_values() -> TestResult {
    let schema = compile(
        "type Small = u8 @range(1, 9) @secret; entry struct Root { value: Small @max(8) }",
    )?;
    let binding = bind::<Root<u8>>(&schema, BindOptions::default())?;
    let session = binding_session()?;
    let root = codec_value(|out| {
        binding
            .plan()
            .get(TypeId::of::<Root<u8>>(), schema.entry(), &session, out)
    })?;
    assert_eq!(
        code(codec_error(|out| root
            .fields
            .by_rust_slot(1, &session, out))?)?,
        15
    );
    let field = root
        .fields
        .as_slice()
        .first()
        .test_expect("root binding should contain one field")?
        .field;
    assert_eq!(
        field,
        present(access(|out| schema.entry().fields(out))?.get(0))?
    );
    let ty = access(|out| field.ty(out))?;
    assert!(access(|out| ty.is_secret(out))?);
    let constraints = access(|out| ty.constraints(out))?;
    assert!(!access(|out| constraints.is_empty(out))?);
    let field_constraints = access(|out| field.constraints(out))?;
    assert!(!access(|out| field_constraints.is_empty(out))?);
    let scalar = codec_value(|out| binding.plan().get(TypeId::of::<u8>(), ty, &session, out))?;
    assert_eq!(scalar.ty, ty);
    Ok(())
}

impl BindingTestSupport {
    pub(crate) fn bind<'schema, T: Typol>(
        schema: &'schema CompiledSchema,
        options: BindOptions,
    ) -> TestResult<TrustedTypedSchema<'schema, T>> {
        bind(schema, options)
    }

    pub(crate) fn bind_error<T: Typol>(
        schema: &CompiledSchema,
        options: BindOptions,
    ) -> TestResult<CodecErrors> {
        bind_error::<T>(schema, options)
    }

    pub(crate) fn codec_value<T>(
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, CodecErrors>) -> OResult<'slot, T>,
    ) -> TestResult<T> {
        codec_value(operation)
    }

    pub(crate) fn codec_error<T>(
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, CodecErrors>) -> OResult<'slot, T>,
    ) -> TestResult<CodecErrors> {
        codec_error(operation)
    }

    pub(crate) fn codec_session() -> TestResult<SessionBudget> {
        codec_session()
    }

    pub(crate) fn compile(source: &str) -> TestResult<CompiledSchema> {
        compile(source)
    }

    pub(crate) fn code(errors: CodecErrors) -> TestResult<u32> {
        code(errors)
    }

    pub(crate) fn structure<'slot, T: 'static>(
        context: &mut DescribeContext<'_>,
        name: &str,
        fields: &[(&str, DescriptorProvider)],
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        structure::<T>(context, name, fields, out)
    }

    pub(crate) fn enum_descriptor<'slot, T: 'static>(
        context: &mut DescribeContext<'_>,
        name: &str,
        shapes: Vec<(&str, VariantShape)>,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        enum_descriptor::<T>(context, name, shapes, out)
    }
}
