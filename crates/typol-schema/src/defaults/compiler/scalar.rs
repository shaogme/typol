use crate::{
    defaults::{DefaultDiagnostic, DefaultNumber},
    diagnostics::BuildFailure,
    types::ResolvedTypeKind,
};
use opeo::{OResult, Out};
use typol_core::{SourceSpan, ValueSource};
use typol_syntax::{BuiltinType, ScalarLiteral};

use super::{BuiltValue, DefaultCompiler};

pub(super) struct ScalarDefaultBuilder;

enum ScalarValue<'a> {
    String(&'a str),
    Boolean(bool),
    Number(DefaultNumber),
}

impl ScalarDefaultBuilder {
    pub(super) fn build<'slot>(
        compiler: &mut DefaultCompiler<'_, '_>,
        source: ScalarLiteral<'_>,
        target: ResolvedTypeKind,
        span: SourceSpan,
        mut out: Out<'slot, '_, BuildFailure>,
    ) -> OResult<'slot, Option<BuiltValue>> {
        let ResolvedTypeKind::Builtin(builtin) = target else {
            compiler.report_invalid(
                DefaultDiagnostic::scalar_type_mismatch(compiler.translator()),
                source.span(),
                out.reborrow(),
            )?;
            return Ok(None);
        };
        let scalar = match source {
            ScalarLiteral::String(value) if builtin == BuiltinType::String => {
                let text = compiler.ast_access(out.reborrow(), |ast_out| value.value(ast_out))?;
                if !compiler.check_string(text, value.span(), out.reborrow())? {
                    return Ok(None);
                }
                ScalarValue::String(text)
            }
            ScalarLiteral::Boolean(value) if builtin == BuiltinType::Bool => {
                ScalarValue::Boolean(value.value())
            }
            ScalarLiteral::Number(value) => {
                match compiler.build_number(value, builtin, out.reborrow())? {
                    Some(value) => ScalarValue::Number(value),
                    None => return Ok(None),
                }
            }
            ScalarLiteral::String(_) => {
                compiler.report_invalid(
                    DefaultDiagnostic::string_type_mismatch(compiler.translator()),
                    source.span(),
                    out.reborrow(),
                )?;
                return Ok(None);
            }
            ScalarLiteral::Boolean(_) => {
                compiler.report_invalid(
                    DefaultDiagnostic::boolean_type_mismatch(compiler.translator()),
                    source.span(),
                    out.reborrow(),
                )?;
                return Ok(None);
            }
        };
        let value = compiler.build_value(
            span,
            |builder, _, mut value_out| {
                let source = ValueSource::SchemaDefault;
                let root = match scalar {
                    ScalarValue::String(text) => {
                        builder.add_string(text, source, value_out.reborrow())?
                    }
                    ScalarValue::Boolean(value) => {
                        builder.add_bool(value, source, value_out.reborrow())?
                    }
                    ScalarValue::Number(DefaultNumber::Signed(value)) => {
                        builder.add_signed(value, source, value_out.reborrow())?
                    }
                    ScalarValue::Number(DefaultNumber::Unsigned(value)) => {
                        builder.add_unsigned(value, source, value_out.reborrow())?
                    }
                    ScalarValue::Number(DefaultNumber::Float32(value)) => {
                        builder.add_float32(value, source, value_out.reborrow())?
                    }
                    ScalarValue::Number(DefaultNumber::Float64(value)) => {
                        builder.add_float64(value, source, value_out.reborrow())?
                    }
                };
                Ok(root)
            },
            out.reborrow(),
        )?;
        let member_spans = compiler.new_vec(out.reborrow())?;
        Ok(Some(compiler.built(value, span, member_spans)))
    }
}
