use crate::{
    Keyword, Symbol, TokenKind,
    error::SyntaxDiagnostic,
    parser::{
        annotation::AnnotationParser,
        context::{ParserControl, ParserState},
        expression::ExpressionParser,
        recovery::{RecoveryBoundary, RecoveryParser},
        type_expr::TypeParser,
        value::DefaultParser,
    },
    storage::{
        Assertion, Declaration, FieldDecl, FieldParams, Id, Name, StructDecl, StructDeclParams,
        TypeAliasDeclParams, VariantDecl, VariantDeclParams,
    },
};
use opeo::{ErrSlot, OResult, Out};
use typol_core::{ChargedVec, Diagnostic, SourceSpan};
use typol_i18n::diagnostic_message;

struct ModuleFrame {
    start: SourceSpan,
    name: Id<Name>,
    declarations: ChargedVec<Declaration>,
    depth: usize,
}

pub(crate) struct DeclarationParser;

impl DeclarationParser {
    pub(crate) fn parse_declaration<'slot>(
        parser: &mut ParserState<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Declaration> {
        let info = ParserControl::peek(parser, out.reborrow())?;
        if info.kind == TokenKind::Keyword(Keyword::Mod) {
            DeclarationParser::parse_module(parser, info.span, out.reborrow())
        } else {
            DeclarationParser::parse_non_module_declaration(parser, out.reborrow())
        }
    }

    fn parse_module<'slot>(
        parser: &mut ParserState<'_>,
        start: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Declaration> {
        let mut frames =
            ParserControl::new_vec(parser, ParserControl::eof_span(parser), out.reborrow())?;
        let first = DeclarationParser::open_module(parser, start, out.reborrow())?;
        ParserControl::try_push(&mut frames, first, start, out.reborrow())?;

        loop {
            parser.execution.consume_with(1, out.reborrow(), |error| {
                SyntaxDiagnostic::budget(error, ParserControl::eof_span(parser))
            })?;

            if parser.stopped {
                return Err(out.reborrow().fail(ParserControl::stopped_error(parser)));
            }
            let info = ParserControl::peek(parser, out.reborrow())?;
            if info.kind == TokenKind::Symbol(Symbol::RightBrace) {
                ParserControl::bump(parser, out.reborrow())?;
                let Some(frame) = frames.pop() else {
                    return Err(out.reborrow().fail(parser_internal_error!(
                        "module-frame-stack-underflow",
                        info.span,
                    )));
                };
                let Some(expected_depth) = frame.depth.checked_sub(1) else {
                    return Err(out.reborrow().fail(parser_internal_error!(
                        "module-frame-depth-underflow",
                        info.span,
                    )));
                };
                if ParserControl::current_depth(parser) != expected_depth {
                    return Err(out.reborrow().fail(parser_internal_error!(
                        "module-frame-depth-mismatch",
                        info.span,
                    )));
                }
                let span = ParserControl::span_from_start(parser, frame.start, out.reborrow())?;
                let module = parser.ast.module_decl_new(
                    frame.name,
                    frame.declarations,
                    span,
                    out.reborrow(),
                )?;
                let declaration = Declaration::Module(module);
                let Some(parent) = frames.last_mut() else {
                    break Ok(declaration);
                };
                ParserControl::try_push(
                    &mut parent.declarations,
                    declaration,
                    frame.start,
                    out.reborrow(),
                )?;
                continue;
            }
            if info.kind == TokenKind::Eof {
                return ParserControl::syntax_failure(
                    parser,
                    info.span,
                    diagnostic_message!(
                        "diagnostic.syntax.module_missing_closing_brace",
                        "module declaration is missing a closing brace"
                    ),
                    out.reborrow(),
                );
            }

            if info.kind == TokenKind::Keyword(Keyword::Mod) {
                let mut member_errors = ErrSlot::<Diagnostic>::new();
                let attempt = match member_errors
                    .try_call(|out| DeclarationParser::open_module(parser, info.span, out))
                {
                    Ok(frame) => Ok(frame),
                    Err(caught) => Err(caught.take()),
                };
                let Some(frame) = RecoveryParser::recover_member(
                    parser,
                    RecoveryBoundary::Module {
                        depth: frames.last().map_or(0, |frame| frame.depth),
                        failed_at: info.span.span().start(),
                    },
                    attempt,
                    out.reborrow(),
                )?
                else {
                    continue;
                };
                ParserControl::try_push(&mut frames, frame, info.span, out.reborrow())?;
            } else if !matches!(
                info.kind,
                TokenKind::Keyword(Keyword::Type | Keyword::Struct | Keyword::Enum)
            ) {
                let failed_at = info.span.span().start();
                let diagnostic = ParserControl::syntax_error(
                    parser,
                    info.span,
                    diagnostic_message!(
                        "diagnostic.syntax.expected_schema_declaration",
                        "expected a Schema declaration here in the module"
                    ),
                    out.reborrow(),
                )?;
                ParserControl::push_diagnostic(parser, diagnostic, out.reborrow())?;
                let Some(frame) = frames.last() else {
                    return Err(out.reborrow().fail(parser_internal_error!(
                        "module-recovery-frame-missing",
                        info.span,
                    )));
                };
                RecoveryParser::recover_module_member(
                    parser,
                    frame.depth,
                    failed_at,
                    out.reborrow(),
                )?;
            } else {
                let mut member_errors = ErrSlot::<Diagnostic>::new();
                let attempt = match member_errors
                    .try_call(|out| DeclarationParser::parse_non_module_declaration(parser, out))
                {
                    Ok(declaration) => Ok(declaration),
                    Err(caught) => Err(caught.take()),
                };
                let Some(declaration) = RecoveryParser::recover_member(
                    parser,
                    RecoveryBoundary::Module {
                        depth: frames.last().map_or(0, |frame| frame.depth),
                        failed_at: info.span.span().start(),
                    },
                    attempt,
                    out.reborrow(),
                )?
                else {
                    continue;
                };
                let Some(parent) = frames.last_mut() else {
                    return Err(out.reborrow().fail(parser_internal_error!(
                        "module-parent-frame-missing",
                        info.span,
                    )));
                };
                ParserControl::try_push(
                    &mut parent.declarations,
                    declaration,
                    info.span,
                    out.reborrow(),
                )?;
            }
        }
    }

    fn open_module<'slot>(
        parser: &mut ParserState<'_>,
        start: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ModuleFrame> {
        ParserControl::expect_keyword(parser, Keyword::Mod, out.reborrow())?;
        let name = ParserControl::take_name(parser, out.reborrow())?;
        ParserControl::expect_symbol(parser, Symbol::LeftBrace, out.reborrow())?;
        Ok(ModuleFrame {
            start,
            name,
            declarations: ParserControl::new_vec(
                parser,
                ParserControl::eof_span(parser),
                out.reborrow(),
            )?,
            depth: ParserControl::current_depth(parser),
        })
    }

    fn parse_non_module_declaration<'slot>(
        parser: &mut ParserState<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Declaration> {
        let info = ParserControl::peek(parser, out.reborrow())?;
        match info.kind {
            TokenKind::Keyword(Keyword::Type) => {
                DeclarationParser::parse_type_alias(parser, info.span, out.reborrow())
            }
            TokenKind::Keyword(Keyword::Struct) => {
                ParserControl::bump(parser, out.reborrow())?;
                Ok(Declaration::Struct(
                    DeclarationParser::parse_struct_after_keyword(
                        parser,
                        info.span,
                        out.reborrow(),
                    )?,
                ))
            }
            TokenKind::Keyword(Keyword::Enum) => {
                DeclarationParser::parse_enum(parser, info.span, out.reborrow())
            }
            _ => ParserControl::syntax_failure(
                parser,
                info.span,
                diagnostic_message!(
                    "diagnostic.syntax.expected_schema_declaration",
                    "expected a Schema declaration here"
                ),
                out.reborrow(),
            ),
        }
    }

    fn parse_type_alias<'slot>(
        parser: &mut ParserState<'_>,
        start: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Declaration> {
        ParserControl::expect_keyword(parser, Keyword::Type, out.reborrow())?;
        let name = ParserControl::take_name(parser, out.reborrow())?;
        ParserControl::expect_symbol(parser, Symbol::Equal, out.reborrow())?;
        let target = TypeParser::parse_type_expr(parser, out.reborrow())?;
        let annotations = AnnotationParser::parse_annotations(parser, out.reborrow())?;
        ParserControl::expect_symbol(parser, Symbol::Semicolon, out.reborrow())?;
        let span = ParserControl::span_from_start(parser, start, out.reborrow())?;
        Ok(Declaration::TypeAlias(parser.ast.type_alias_decl_new(
            TypeAliasDeclParams {
                name,
                target,
                annotations,
                span,
            },
            out.reborrow(),
        )?))
    }

    pub(crate) fn parse_struct_after_keyword<'slot>(
        parser: &mut ParserState<'_>,
        start: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<StructDecl>> {
        let name = ParserControl::take_name(parser, out.reborrow())?;
        ParserControl::expect_symbol(parser, Symbol::LeftBrace, out.reborrow())?;
        let fields = DeclarationParser::parse_field_block(parser, out.reborrow())?;
        let assertions = DeclarationParser::parse_assertions(parser, out.reborrow())?;
        let span = ParserControl::span_from_start(parser, start, out.reborrow())?;
        parser.ast.struct_decl_new(
            StructDeclParams {
                name,
                fields,
                assertions,
                span,
            },
            out.reborrow(),
        )
    }

    fn parse_field_block<'slot>(
        parser: &mut ParserState<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ChargedVec<Id<FieldDecl>>> {
        let base_depth = ParserControl::current_depth(parser);
        let mut fields =
            ParserControl::new_vec(parser, ParserControl::eof_span(parser), out.reborrow())?;

        loop {
            parser.execution.consume_with(1, out.reborrow(), |error| {
                SyntaxDiagnostic::budget(error, ParserControl::eof_span(parser))
            })?;

            if parser.stopped {
                return Err(out.reborrow().fail(ParserControl::stopped_error(parser)));
            }
            let info = ParserControl::peek(parser, out.reborrow())?;
            if info.kind == TokenKind::Symbol(Symbol::RightBrace) {
                ParserControl::bump(parser, out.reborrow())?;
                break Ok(fields);
            }
            if info.kind == TokenKind::Eof {
                return ParserControl::syntax_failure(
                    parser,
                    info.span,
                    diagnostic_message!(
                        "diagnostic.syntax.field_list_missing_closing_brace",
                        "field list is missing a closing brace"
                    ),
                    out.reborrow(),
                );
            }

            let mut member_errors = ErrSlot::<Diagnostic>::new();
            let attempt =
                match member_errors.try_call(|out| DeclarationParser::parse_field(parser, out)) {
                    Ok(field) => Ok(field),
                    Err(caught) => Err(caught.take()),
                };
            let Some(field) = RecoveryParser::recover_member(
                parser,
                RecoveryBoundary::Separated { depth: base_depth },
                attempt,
                out.reborrow(),
            )?
            else {
                continue;
            };
            ParserControl::try_push(&mut fields, field, info.span, out.reborrow())?;
            let separator = ParserControl::peek(parser, out.reborrow())?;
            match separator.kind {
                TokenKind::Symbol(Symbol::Comma | Symbol::RightBrace) => {}
                _ => {
                    let diagnostic = ParserControl::syntax_error(
                        parser,
                        separator.span,
                        diagnostic_message!(
                            "diagnostic.syntax.fields_require_commas",
                            "fields must be separated by commas"
                        ),
                        out.reborrow(),
                    )?;
                    ParserControl::push_diagnostic(parser, diagnostic, out.reborrow())?;
                    RecoveryParser::recover_fields(parser, base_depth, out.reborrow())?;
                }
            }
            if separator.kind == TokenKind::Symbol(Symbol::Comma) {
                ParserControl::bump(parser, out.reborrow())?;
            }

            if parser.stopped {
                return Err(out.reborrow().fail(ParserControl::stopped_error(parser)));
            }
        }
    }

    fn parse_field<'slot>(
        parser: &mut ParserState<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<FieldDecl>> {
        let start = ParserControl::peek(parser, out.reborrow())?.span;
        let name = ParserControl::take_name(parser, out.reborrow())?;
        ParserControl::expect_symbol(parser, Symbol::Colon, out.reborrow())?;
        let ty = TypeParser::parse_type_expr(parser, out.reborrow())?;
        let annotations = AnnotationParser::parse_annotations(parser, out.reborrow())?;
        let default = if ParserControl::eat_symbol(parser, Symbol::Equal, out.reborrow())?.is_some()
        {
            Some(DefaultParser::parse_default_value(parser, out.reborrow())?)
        } else {
            None
        };
        let span = ParserControl::span_from_start(parser, start, out.reborrow())?;
        parser.ast.field_decl_new(
            FieldParams {
                name,
                ty,
                annotations,
                default,
                span,
            },
            out.reborrow(),
        )
    }

    fn parse_enum<'slot>(
        parser: &mut ParserState<'_>,
        start: SourceSpan,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Declaration> {
        ParserControl::expect_keyword(parser, Keyword::Enum, out.reborrow())?;
        let name = ParserControl::take_name(parser, out.reborrow())?;
        ParserControl::expect_symbol(parser, Symbol::LeftBrace, out.reborrow())?;
        let base_depth = ParserControl::current_depth(parser);
        let mut variants =
            ParserControl::new_vec(parser, ParserControl::eof_span(parser), out.reborrow())?;
        let mut variant_failed = false;

        loop {
            parser.execution.consume_with(1, out.reborrow(), |error| {
                SyntaxDiagnostic::budget(error, ParserControl::eof_span(parser))
            })?;

            if parser.stopped {
                return Err(out.reborrow().fail(ParserControl::stopped_error(parser)));
            }
            let info = ParserControl::peek(parser, out.reborrow())?;
            if info.kind == TokenKind::Symbol(Symbol::RightBrace) {
                if variants.is_empty() && !variant_failed {
                    return ParserControl::syntax_failure(
                        parser,
                        info.span,
                        diagnostic_message!(
                            "diagnostic.syntax.enum_requires_variant",
                            "an enum must have at least one variant"
                        ),
                        out.reborrow(),
                    );
                }
                ParserControl::bump(parser, out.reborrow())?;
                break;
            }
            if info.kind == TokenKind::Eof {
                return ParserControl::syntax_failure(
                    parser,
                    info.span,
                    diagnostic_message!(
                        "diagnostic.syntax.enum_missing_closing_brace",
                        "enum declaration is missing a closing brace"
                    ),
                    out.reborrow(),
                );
            }

            let mut member_errors = ErrSlot::<Diagnostic>::new();
            let attempt =
                match member_errors.try_call(|out| DeclarationParser::parse_variant(parser, out)) {
                    Ok(variant) => Ok(variant),
                    Err(caught) => Err(caught.take()),
                };
            let Some(variant) = RecoveryParser::recover_member(
                parser,
                RecoveryBoundary::Separated { depth: base_depth },
                attempt,
                out.reborrow(),
            )?
            else {
                variant_failed = true;
                continue;
            };
            ParserControl::try_push(&mut variants, variant, info.span, out.reborrow())?;
            let separator = ParserControl::peek(parser, out.reborrow())?;
            match separator.kind {
                TokenKind::Symbol(Symbol::Comma | Symbol::RightBrace) => {}
                _ => {
                    let diagnostic = ParserControl::syntax_error(
                        parser,
                        separator.span,
                        diagnostic_message!(
                            "diagnostic.syntax.enum_variants_require_commas",
                            "enum variants must be separated by commas"
                        ),
                        out.reborrow(),
                    )?;
                    ParserControl::push_diagnostic(parser, diagnostic, out.reborrow())?;
                    RecoveryParser::recover_variants(parser, base_depth, out.reborrow())?;
                }
            }
            if separator.kind == TokenKind::Symbol(Symbol::Comma) {
                ParserControl::bump(parser, out.reborrow())?;
            }
            if parser.stopped {
                return Err(out.reborrow().fail(ParserControl::stopped_error(parser)));
            }
        }

        let span = ParserControl::span_from_start(parser, start, out.reborrow())?;
        Ok(Declaration::Enum(parser.ast.enum_decl_new(
            name,
            variants,
            span,
            out.reborrow(),
        )?))
    }

    fn parse_variant<'slot>(
        parser: &mut ParserState<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<VariantDecl>> {
        let start = ParserControl::peek(parser, out.reborrow())?.span;
        let name = ParserControl::take_name(parser, out.reborrow())?;
        let payload = match ParserControl::peek(parser, out.reborrow())?.kind {
            TokenKind::Symbol(Symbol::LeftParen) => {
                let open = ParserControl::bump(parser, out.reborrow())?.span;
                let ty = TypeParser::parse_type_expr(parser, out.reborrow())?;
                let annotations = AnnotationParser::parse_annotations(parser, out.reborrow())?;
                let close =
                    ParserControl::expect_symbol(parser, Symbol::RightParen, out.reborrow())?;
                let span = ParserControl::span_between(parser, open, close.span, out.reborrow())?;
                parser
                    .ast
                    .variant_payload_single(ty, annotations, span, out.reborrow())?
            }
            TokenKind::Symbol(Symbol::LeftBrace) => {
                let open = ParserControl::bump(parser, out.reborrow())?.span;
                let fields = DeclarationParser::parse_field_block(parser, out.reborrow())?;
                let span = ParserControl::span_from_start(parser, open, out.reborrow())?;
                parser
                    .ast
                    .variant_payload_structure(fields, span, out.reborrow())?
            }
            _ => {
                let span = parser.ast.span(name, out.reborrow())?;
                parser.ast.variant_payload_unit(span, out.reborrow())?
            }
        };
        let assertions = DeclarationParser::parse_assertions(parser, out.reborrow())?;
        let span = ParserControl::span_from_start(parser, start, out.reborrow())?;
        parser.ast.variant_decl_new(
            VariantDeclParams {
                name,
                payload,
                assertions,
                span,
            },
            out.reborrow(),
        )
    }

    fn parse_assertions<'slot>(
        parser: &mut ParserState<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, ChargedVec<Id<Assertion>>> {
        let mut assertions =
            ParserControl::new_vec(parser, ParserControl::eof_span(parser), out.reborrow())?;
        loop {
            parser.execution.consume_with(1, out.reborrow(), |error| {
                SyntaxDiagnostic::budget(error, ParserControl::eof_span(parser))
            })?;

            let info = ParserControl::peek(parser, out.reborrow())?;
            if info.kind != TokenKind::Symbol(Symbol::At) {
                break Ok(assertions);
            }
            let next = ParserControl::peek_next(parser, out.reborrow())?;
            if next.kind != TokenKind::Identifier {
                return ParserControl::syntax_failure(
                    parser,
                    next.span,
                    diagnostic_message!(
                        "diagnostic.syntax.assertion_annotation_name_required",
                        "assertion annotation requires a name"
                    ),
                    out.reborrow(),
                );
            }
            let execution = parser.execution.try_clone_with(out.reborrow(), |error| {
                SyntaxDiagnostic::budget(error, info.span)
            })?;
            let token = ParserControl::next_token(parser, out.reborrow())?;
            let token_span = token.span();
            let Some(name) = token.text() else {
                return Err(out.reborrow().fail(parser_internal_error!(
                    "assertion-name-token-text-missing",
                    token_span,
                )));
            };
            if !execution
                .compare_text(name, "assert", out.reborrow())?
                .is_eq()
            {
                break Ok(assertions);
            }
            let assertion = DeclarationParser::parse_assertion(parser, out.reborrow())?;
            ParserControl::try_push(&mut assertions, assertion, info.span, out.reborrow())?;
        }
    }

    fn parse_assertion<'slot>(
        parser: &mut ParserState<'_>,
        mut out: Out<'slot, '_, Diagnostic>,
    ) -> OResult<'slot, Id<Assertion>> {
        let start = ParserControl::expect_symbol(parser, Symbol::At, out.reborrow())?.span;
        let name = ParserControl::take_name(parser, out.reborrow())?;
        if !ParserControl::name_is(parser, name, "assert", out.reborrow())? {
            let span = parser.ast.span(name, out.reborrow())?;
            return ParserControl::syntax_failure(
                parser,
                span,
                diagnostic_message!(
                    "diagnostic.syntax.expected_assert_annotation",
                    "expected @assert here"
                ),
                out.reborrow(),
            );
        }
        ParserControl::expect_symbol(parser, Symbol::LeftParen, out.reborrow())?;
        let expression = ExpressionParser::parse_expression(parser, out.reborrow())?;
        ParserControl::expect_symbol(parser, Symbol::Comma, out.reborrow())?;
        let message = ParserControl::take_string(parser, out.reborrow())?;
        ParserControl::expect_symbol(parser, Symbol::RightParen, out.reborrow())?;
        let span = ParserControl::span_from_start(parser, start, out.reborrow())?;
        parser
            .ast
            .assertion_new(expression, message, span, out.reborrow())
    }
}
