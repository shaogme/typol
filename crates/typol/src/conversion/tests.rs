mod budget_and_allocation;
mod diagnostics;

use crate::{
    BindingTestRoot as Root, BindingTestSupport,
    binding_impl::TypedSchema,
    conversion::{
        Conversion, ConversionContext, ConversionPayload, ConversionValue, ConvertedValue,
        OrderedMap,
    },
    descriptor::{
        DescribeContext, DescriptorNode, DescriptorProvider, TypeDescriptor, Typol, VariantShape,
    },
    diagnostics::{CodecErrors, CodecStage},
    options::{BindOptions, ConversionLimits},
    session::SessionBudget,
    test_support::{TestOpeo, TestValues},
    value::EnumPayload,
};
use opeo::{OResult, Out};
use std::{cell::Cell, collections::HashMap, rc::Rc};
use typol_core::{ConfigValue, DiagnosticCode, ValueId, ValueKind, ValueSource};
use typol_model::BuiltinType;
use typol_test_support::{TestExpect, TestExtract, TestResult, test_verify_eq as assert_eq};

pub(crate) struct ConversionTestSupport;

impl ConversionTestSupport {
    pub(crate) fn dynamic(value: ConfigValue) -> TestResult<ConfigValue> {
        dynamic(value)
    }

    pub(crate) fn decode_error<T: Typol>(
        binding: &TypedSchema<'_, T>,
        value: &ConfigValue,
        session: &SessionBudget,
        limits: ConversionLimits,
    ) -> TestResult<CodecErrors> {
        decode_error(binding, value, session, limits)
    }
}

fn roundtrip<T: Typol>(dsl: &str, value: T) -> TestResult<T> {
    let schema = BindingTestSupport::compile(dsl)?;
    let binding = BindingTestSupport::bind::<T>(&schema, BindOptions::default())?;
    let session = BindingTestSupport::codec_session()?;
    let converted = encode(&binding, &value, &session, ConversionLimits::default())?;
    assert_eq!(converted.value().source(), ValueSource::RustValue);
    decode(
        &binding,
        converted.value(),
        &session,
        ConversionLimits::default(),
    )
}

fn encode<T: Typol>(
    binding: &TypedSchema<'_, T>,
    value: &T,
    session: &SessionBudget,
    limits: ConversionLimits,
) -> TestResult<ConvertedValue> {
    BindingTestSupport::codec_value(|out| Conversion::encode(binding, value, session, limits, out))
}

fn decode<T: Typol>(
    binding: &TypedSchema<'_, T>,
    value: &ConfigValue,
    session: &SessionBudget,
    limits: ConversionLimits,
) -> TestResult<T> {
    BindingTestSupport::codec_value(|out| Conversion::decode(binding, value, session, limits, out))
}

fn decode_error<T: Typol>(
    binding: &TypedSchema<'_, T>,
    value: &ConfigValue,
    session: &SessionBudget,
    limits: ConversionLimits,
) -> TestResult<CodecErrors> {
    BindingTestSupport::codec_error(|out| Conversion::decode(binding, value, session, limits, out))
}

fn encode_error<T: Typol>(
    binding: &TypedSchema<'_, T>,
    value: &T,
    session: &SessionBudget,
    limits: ConversionLimits,
) -> TestResult<CodecErrors> {
    BindingTestSupport::codec_error(|out| Conversion::encode(binding, value, session, limits, out))
}

#[test]
fn scalars_preserve_extremes_and_float_bits() -> TestResult {
    macro_rules! integer {
        ($($rust:ty => $dsl:literal),* $(,)?) => { $(
            for value in [<$rust>::MIN, <$rust>::MAX] {
                let actual = roundtrip(concat!("entry struct Root { value: ", $dsl, " }"), Root::new(value))?;
                assert_eq!(actual.into_value(), value);
            }
        )* };
    }
    integer!(i8 => "i8", i16 => "i16", i32 => "i32", i64 => "i64", i128 => "i128", u8 => "u8", u16 => "u16", u32 => "u32", u64 => "u64", u128 => "u128");
    for value in [0.0f32, -0.0, f32::from_bits(1), f32::MAX, f32::MIN] {
        assert_eq!(
            roundtrip("entry struct Root { value: f32 }", Root::new(value))?
                .into_value()
                .to_bits(),
            value.to_bits()
        );
    }
    for value in [0.0f64, -0.0, f64::from_bits(1), f64::MAX, f64::MIN] {
        assert_eq!(
            roundtrip("entry struct Root { value: f64 }", Root::new(value))?
                .into_value()
                .to_bits(),
            value.to_bits()
        );
    }
    for value in [true, false] {
        assert_eq!(
            roundtrip("entry struct Root { value: bool }", Root::new(value))?.into_value(),
            value
        );
    }
    let value = "é\0secret\n".to_string();
    assert_eq!(
        roundtrip(
            "entry struct Root { value: string }",
            Root::new(value.clone())
        )?
        .into_value(),
        value
    );
    Ok(())
}

fn dynamic(value: ConfigValue) -> TestResult<ConfigValue> {
    TestOpeo::value(|out| {
        TestValues::try_structure(vec![TestValues::field("value".into(), value)], out)
    })
}

#[test]
fn scalar_bad_representations_and_nonfinite_values_fail_atomically() -> TestResult {
    let schema = BindingTestSupport::compile("entry struct Root { value: u8 }")?;
    let binding = BindingTestSupport::bind::<Root<u8>>(&schema, BindOptions::default())?;
    for value in [
        TestValues::unsigned(256),
        TestValues::signed(1),
        TestValues::boolean(true),
    ] {
        let session = BindingTestSupport::codec_session()?;
        assert_eq!(
            BindingTestSupport::code(decode_error(
                &binding,
                &dynamic(value)?,
                &session,
                ConversionLimits::default(),
            )?)?,
            15
        );
    }
    let schema = BindingTestSupport::compile("entry struct Root { value: f32 }")?;
    let binding = BindingTestSupport::bind::<Root<f32>>(&schema, BindOptions::default())?;
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let session = BindingTestSupport::codec_session()?;
        assert_eq!(
            BindingTestSupport::code(encode_error(
                &binding,
                &Root::new(value),
                &session,
                ConversionLimits::default(),
            )?)?,
            15
        );
        assert_eq!(
            BindingTestSupport::code(decode_error(
                &binding,
                &dynamic(TestValues::float32(value))?,
                &session,
                ConversionLimits::default(),
            )?)?,
            15
        );
    }
    let session = BindingTestSupport::codec_session()?;
    assert_eq!(
        BindingTestSupport::code(decode_error(
            &binding,
            &dynamic(TestValues::float64(1.0))?,
            &session,
            ConversionLimits::default(),
        )?)?,
        15
    );
    Ok(())
}

#[test]
fn options_arrays_and_nested_containers_construct_owned_targets() -> TestResult {
    for value in [None, Some(255u8)] {
        assert_eq!(
            roundtrip(
                "entry struct Root { value: option<u8> = 9 }",
                Root::new(value)
            )?
            .into_value(),
            value
        );
    }
    assert_eq!(
        roundtrip(
            "entry struct Root { value: [u8; 0] }",
            Root::new([] as [u8; 0])
        )?
        .into_value(),
        [] as [u8; 0]
    );
    assert_eq!(
        roundtrip(
            "entry struct Root { value: [u8; 2] }",
            Root::new([0u8, 255])
        )?
        .into_value(),
        [0, 255]
    );
    for value in [vec![], vec![Some(vec![1u8, 2]), None, Some(vec![])]] {
        assert_eq!(
            roundtrip(
                "entry struct Root { value: [option<[u8]>] }",
                Root::new(value.clone())
            )?
            .into_value(),
            value
        );
    }
    let schema = BindingTestSupport::compile("entry struct Root { value: [u8; 2] }")?;
    let binding = BindingTestSupport::bind::<Root<[u8; 2]>>(&schema, BindOptions::default())?;
    let session = BindingTestSupport::codec_session()?;
    assert_eq!(
        BindingTestSupport::code(decode_error(
            &binding,
            &dynamic(TestValues::array_unmetered(vec![TestValues::unsigned(1)]),)?,
            &session,
            ConversionLimits::default(),
        )?)?,
        15
    );
    Ok(())
}

#[derive(Debug, PartialEq)]
pub(crate) struct Pair {
    second: String,
    first: u8,
}

impl Typol for Pair {
    fn describe<'slot, 'borrow>(
        context: &mut DescribeContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        BindingTestSupport::structure::<Self>(
            context,
            "Renamed",
            &[
                ("b", DescriptorProvider::of::<String>()),
                ("a", DescriptorProvider::of::<u8>()),
            ],
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
            |slot, child, child_out| match slot {
                0 => child.encode_here(&self.second, child_out),
                1 => child.encode_here(&self.first, child_out),
                _ => child.reject_with_path(child_out),
            },
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
        let second = context.read_field(fields, 0, out.reborrow())?;
        let first = context.read_field(fields, 1, out.reborrow())?;
        Ok(Self { second, first })
    }
}

struct Recursive;

impl Typol for Recursive {
    fn describe<'slot>(
        context: &mut DescribeContext<'_>,
        out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        BindingTestSupport::structure::<Self>(
            context,
            "Recursive",
            &[("value", DescriptorProvider::of::<u8>())],
            out,
        )
    }

    fn to_config_value<'slot>(
        &self,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        context.start::<Self>(out.reborrow())?;
        Self::to_config_value(self, context, out)
    }

    fn from_config_value<'slot>(
        value: ConversionValue<'_>,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        context.start::<Self>(out.reborrow())?;
        if !matches!(value.view().as_ref(), ValueKind::Structure(_)) {
            return context.reject_with_path(out.reborrow());
        }
        Self::from_config_value(value, context, out)
    }
}

#[test]
fn handwritten_recursive_conversion_is_stopped_by_the_node_dispatcher() -> TestResult {
    let schema = BindingTestSupport::compile("entry struct Recursive { value: u8 }")?;
    let binding = BindingTestSupport::bind::<Recursive>(&schema, BindOptions::default())?;
    let session = BindingTestSupport::codec_session()?;
    let value = Recursive;
    assert_eq!(
        BindingTestSupport::code(encode_error(
            &binding,
            &value,
            &session,
            ConversionLimits::default(),
        )?)?,
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED.get()
    );
    let config = dynamic(TestValues::unsigned(1))?;
    assert_eq!(
        BindingTestSupport::code(decode_error(
            &binding,
            &config,
            &session,
            ConversionLimits::default(),
        )?)?,
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED.get()
    );
    Ok(())
}

#[test]
fn renamed_reordered_nested_fields_follow_schema_order() -> TestResult {
    let schema = BindingTestSupport::compile(
        "struct Renamed { a: u8, b: string } entry struct Root { value: Renamed }",
    )?;
    let binding = BindingTestSupport::bind::<Root<Pair>>(&schema, BindOptions::default())?;
    let value = Root::new(Pair {
        first: 42,
        second: "preserved secret".into(),
    });
    let session = BindingTestSupport::codec_session()?;
    let limits = ConversionLimits::new(100, 2).test_value()?;
    let converted = encode(&binding, &value, &session, limits)?;
    let ValueKind::Structure(mut root) = converted.value().as_ref() else {
        return Err("encoded result should be a struct".into());
    };
    let root_value = root
        .next()
        .test_expect("root struct should contain one field")?
        .value();
    let ValueKind::Structure(fields) = root_value.as_ref() else {
        return Err("nested field should be a struct".into());
    };
    assert_eq!(
        fields
            .map(|field| {
                field
                    .as_named()
                    .map(|named| named.name())
                    .test_expect("struct fields must be named")
            })
            .collect::<TestResult<Vec<_>>>()?,
        ["a", "b"]
    );
    assert_eq!(
        decode(&binding, converted.value(), &session, limits)?,
        value
    );
    let shallow_limits = ConversionLimits::new(100, 1).test_value()?;
    assert_eq!(
        BindingTestSupport::code(encode_error(&binding, &value, &session, shallow_limits,)?)?,
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED.get()
    );
    assert_eq!(
        BindingTestSupport::code(decode_error(
            &binding,
            converted.value(),
            &session,
            shallow_limits,
        )?)?,
        DiagnosticCode::RESOURCE_LIMIT_EXCEEDED.get()
    );
    Ok(())
}

#[test]
fn hash_map_uses_unicode_key_order_and_counts_keys_as_nodes() -> TestResult {
    let entries = [("ö", 3u8), ("a", 1), ("é", 2), ("", 0)];
    let hash: HashMap<_, _> = entries
        .iter()
        .rev()
        .map(|(key, value)| (key.to_string(), *value))
        .collect();
    assert_eq!(
        roundtrip(
            "entry struct Root { value: map<string, u8> }",
            Root::new(hash.clone())
        )?
        .into_value(),
        hash
    );
    let schema = BindingTestSupport::compile("entry struct Root { value: map<string, u8> }")?;
    let binding =
        BindingTestSupport::bind::<Root<HashMap<String, u8>>>(&schema, BindOptions::default())?;
    let session = BindingTestSupport::codec_session()?;
    let converted = encode(
        &binding,
        &Root::new(hash),
        &session,
        ConversionLimits::new(10, 2).test_value()?,
    )?;
    let ValueKind::Structure(mut root) = converted.value().as_ref() else {
        return Err("encoded result should be a struct".into());
    };
    let root_value = root
        .next()
        .test_expect("root struct should contain one field")?
        .value();
    let ValueKind::Map(entries) = root_value.as_ref() else {
        return Err("map field should be a Map".into());
    };
    assert_eq!(
        entries
            .map(|entry| {
                entry
                    .as_named()
                    .map(|named| named.name())
                    .test_expect("map entries must be named")
            })
            .collect::<TestResult<Vec<_>>>()?,
        ["", "a", "é", "ö"]
    );
    let session = BindingTestSupport::codec_session()?;
    assert_eq!(
        BindingTestSupport::code(decode_error(
            &binding,
            converted.value(),
            &session,
            ConversionLimits::new(9, 2).test_value()?,
        )?)?,
        1
    );
    Ok(())
}

#[test]
fn ordered_map_sorts_decoded_keys() -> TestResult {
    let schema = BindingTestSupport::compile("entry struct Root { value: map<string, u8> }")?;
    let binding =
        BindingTestSupport::bind::<Root<OrderedMap<u8>>>(&schema, BindOptions::default())?;
    let input = dynamic(TestValues::map_unmetered(vec![
        TestValues::map_entry("z".into(), TestValues::unsigned(3)),
        TestValues::map_entry("a".into(), TestValues::unsigned(1)),
        TestValues::map_entry("m".into(), TestValues::unsigned(2)),
    ]))?;
    let session = BindingTestSupport::codec_session()?;
    let decoded = decode(&binding, &input, &session, ConversionLimits::default())?;
    assert_eq!(
        decoded
            .into_value()
            .iter()
            .map(|(key, value)| (key, *value))
            .collect::<Vec<_>>(),
        [("a", 1), ("m", 2), ("z", 3)]
    );

    Ok(())
}

#[derive(Debug, PartialEq)]
pub(crate) enum Choice {
    Object { name: String, count: u8 },
    Number(i128),
    Unit,
}

impl Typol for Choice {
    fn describe<'slot>(
        context: &mut DescribeContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        let mut fields = context.session().budget_codec(
            CodecStage::Binding,
            |budget_out| context.session().workspace(budget_out),
            out.reborrow(),
        )?;
        context.session().budget_codec(
            CodecStage::Binding,
            |budget_out| fields.try_reserve_exact(2, budget_out),
            out.reborrow(),
        )?;
        let name = context.name("name", out.reborrow())?;
        let name_field = context.insert(
            DescriptorNode::Field {
                name,
                value: DescriptorProvider::of::<String>(),
            },
            context.depth(),
            out.reborrow(),
        )?;
        context.session().budget_codec(
            CodecStage::Binding,
            |budget_out| fields.try_push(name_field, budget_out),
            out.reborrow(),
        )?;
        let count = context.name("count", out.reborrow())?;
        let count_field = context.insert(
            DescriptorNode::Field {
                name: count,
                value: DescriptorProvider::of::<u8>(),
            },
            context.depth(),
            out.reborrow(),
        )?;
        context.session().budget_codec(
            CodecStage::Binding,
            |budget_out| fields.try_push(count_field, budget_out),
            out.reborrow(),
        )?;
        BindingTestSupport::enum_descriptor::<Self>(
            context,
            "Choice",
            vec![
                ("Object", VariantShape::Structure(fields)),
                (
                    "Number",
                    VariantShape::Single(DescriptorProvider::of::<i128>()),
                ),
                ("Unit", VariantShape::Unit),
            ],
            out.reborrow(),
        )
    }

    fn to_config_value<'slot>(
        &self,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        context.start::<Self>(out.reborrow())?;
        match self {
            Self::Object { name, count } => context.structure_variant(
                0,
                |field_slot, child, child_out| match field_slot {
                    0 => child.encode_here(name, child_out),
                    1 => child.encode_here(count, child_out),
                    _ => child.reject_with_path(child_out),
                },
                out.reborrow(),
            ),
            Self::Number(value) => context.single_variant(1, value, out.reborrow()),
            Self::Unit => context.unit_variant(2, out.reborrow()),
        }
    }

    fn from_config_value<'slot>(
        value: ConversionValue<'_>,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        context.start::<Self>(out.reborrow())?;
        let input = context.read_enumeration(value, out.reborrow())?;
        match (input.rust_slot, input.payload) {
            (0, ConversionPayload::Structure(fields)) => context.variant(
                0,
                |context, _, mut variant_out| {
                    let name = context.read_field(fields, 0, variant_out.reborrow())?;
                    let count = context.read_field(fields, 1, variant_out.reborrow())?;
                    Ok(Self::Object { name, count })
                },
                out.reborrow(),
            ),
            (1, ConversionPayload::Single(value)) => {
                let value = context.read_single(1, value, out.reborrow())?;
                Ok(Self::Number(value))
            }
            (2, ConversionPayload::Unit) => Ok(Self::Unit),
            _ => context.reject_with_path(out.reborrow()),
        }
    }
}

pub(crate) const ENUM_SCHEMA: &str = "enum Choice { Unit, Number(i128), Object { count: u8, name: string } } entry struct Root { value: Choice }";

#[test]
fn three_enum_payloads_roundtrip_with_variant_and_field_reordering() -> TestResult {
    for value in [
        Choice::Unit,
        Choice::Number(i128::MIN),
        Choice::Object {
            count: 255,
            name: "secret".into(),
        },
    ] {
        let expected = match &value {
            Choice::Unit => Choice::Unit,
            Choice::Number(value) => Choice::Number(*value),
            Choice::Object { count, name } => Choice::Object {
                count: *count,
                name: name.clone(),
            },
        };
        assert_eq!(
            roundtrip(ENUM_SCHEMA, Root::new(value))?.into_value(),
            expected
        );
    }
    Ok(())
}

#[test]
fn missing_extra_duplicate_variants_and_payload_mismatches_are_rejected() -> TestResult {
    for declaration in [
        "enum Choice { Unit, Number(i128) }",
        "enum Choice { Unit, Number(i128), Object { count: u8, name: string }, Extra }",
        "enum Choice { Unit, Number(u128), Object { count: u8, name: string } }",
        "enum Choice { Unit, Number(i128), Object(u8) }",
        "enum Choice { Unit(u8), Number(i128), Object { count: u8, name: string } }",
        "enum Choice { Unit, Number(i128), Object { count: u8, renamed: string } }",
    ] {
        let schema = BindingTestSupport::compile(&format!(
            "{declaration} entry struct Root {{ value: Choice }}"
        ))?;
        assert_eq!(
            BindingTestSupport::code(BindingTestSupport::bind_error::<Root<Choice>>(
                &schema,
                BindOptions::default()
            )?)?,
            14
        );
    }
    let schema = BindingTestSupport::compile(ENUM_SCHEMA)?;
    let binding = BindingTestSupport::bind::<Root<Choice>>(&schema, BindOptions::default())?;
    for value in [
        TestValues::enumeration_unmetered("Missing".into(), EnumPayload::unit()),
        TestValues::enumeration_unmetered("Number".into(), EnumPayload::unit()),
    ] {
        let session = BindingTestSupport::codec_session()?;
        assert_eq!(
            BindingTestSupport::code(decode_error(
                &binding,
                &dynamic(value)?,
                &session,
                ConversionLimits::default(),
            )?)?,
            15
        );
    }
    Ok(())
}

thread_local! {
    static LIVE: Rc<Cell<usize>> = Rc::new(Cell::new(0));
}

struct Tracked {
    live: Rc<Cell<usize>>,
}

impl Drop for Tracked {
    fn drop(&mut self) {
        self.live.set(self.live.get() - 1);
    }
}

impl Typol for Tracked {
    fn describe<'slot, 'borrow>(
        context: &mut DescribeContext<'_>,
        out: Out<'slot, 'borrow, CodecErrors>,
    ) -> OResult<'slot, TypeDescriptor> {
        context.scalar::<Self>(BuiltinType::U8, context.depth(), out)
    }

    fn to_config_value<'slot>(
        &self,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, ValueId> {
        context.start::<Self>(out.reborrow())?;
        context.unsigned(1, out.reborrow())
    }

    fn from_config_value<'slot>(
        value: ConversionValue<'_>,
        context: &mut ConversionContext<'_>,
        mut out: Out<'slot, '_, CodecErrors>,
    ) -> OResult<'slot, Self> {
        context.start::<Self>(out.reborrow())?;
        if !matches!(value.view().as_ref(), ValueKind::Unsigned(1)) {
            return context.reject_with_path(out.reborrow());
        }
        Ok(LIVE.with(|live| {
            live.set(live.get() + 1);
            Self { live: live.clone() }
        }))
    }
}

#[test]
fn partial_arrays_vectors_and_maps_drop_every_constructed_item() -> TestResult {
    let session = BindingTestSupport::codec_session()?;
    let array = dynamic(TestValues::array_unmetered(vec![
        TestValues::unsigned(1),
        TestValues::unsigned(2),
    ]))?;
    let schema = BindingTestSupport::compile("entry struct Root { value: [u8; 2] }")?;
    let binding = BindingTestSupport::bind::<Root<[Tracked; 2]>>(&schema, BindOptions::default())?;
    assert_eq!(
        BindingTestSupport::code(decode_error(
            &binding,
            &array,
            &session,
            ConversionLimits::default()
        )?)?,
        15
    );
    assert_eq!(LIVE.with(|live| live.get()), 0);
    let schema = BindingTestSupport::compile("entry struct Root { value: [u8] }")?;
    let binding = BindingTestSupport::bind::<Root<Vec<Tracked>>>(&schema, BindOptions::default())?;
    assert_eq!(
        BindingTestSupport::code(decode_error(
            &binding,
            &array,
            &session,
            ConversionLimits::default()
        )?)?,
        15
    );
    assert_eq!(LIVE.with(|live| live.get()), 0);
    let value = dynamic(TestOpeo::value(|out| {
        TestValues::try_map(
            vec![
                TestValues::map_entry("a".into(), TestValues::unsigned(1)),
                TestValues::map_entry("b".into(), TestValues::unsigned(2)),
            ],
            out,
        )
    })?)?;
    let schema = BindingTestSupport::compile("entry struct Root { value: map<string, u8> }")?;
    let binding = BindingTestSupport::bind::<Root<HashMap<String, Tracked>>>(
        &schema,
        BindOptions::default(),
    )?;
    assert_eq!(
        BindingTestSupport::code(decode_error(
            &binding,
            &value,
            &session,
            ConversionLimits::default()
        )?)?,
        15
    );
    assert_eq!(LIVE.with(|live| live.get()), 0);
    let schema = BindingTestSupport::compile("entry struct Root { value: map<string, u8> }")?;
    let binding =
        BindingTestSupport::bind::<Root<OrderedMap<Tracked>>>(&schema, BindOptions::default())?;
    assert_eq!(
        BindingTestSupport::code(decode_error(
            &binding,
            &value,
            &session,
            ConversionLimits::default()
        )?)?,
        15
    );
    assert_eq!(LIVE.with(|live| live.get()), 0);
    Ok(())
}
