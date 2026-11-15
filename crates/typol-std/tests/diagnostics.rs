use std::{cell::Cell, error::Error, io, path::Path, rc::Rc, slice::from_ref};
use typol_i18n::{Language, Translator, t};

use opeo::{OResult, Out};
use typol_opeo_test_support::{TestOpeo, ensure, ensure_eq, ensure_ne};

use typol_config::{
    ConfigParseErrors, ConfigParseOptions, ConfigParseRequest, ConfigParser, ConfigValueErrors,
    Environment, EnvironmentError, EnvironmentReadContext,
};
use typol_core::{
    BudgetError, Diagnostic, DiagnosticCode, DiagnosticContext as CoreDiagnosticContext,
    DiagnosticContextDescriptor, DiagnosticFailure, DiagnosticLabelInput, ExecutionBudget,
    ExecutionLimits, InstancePath, InternalError, RelatedLocationRole, ResourceLimit, SliceSource,
    SourceExcerptSet, SourceId, SourceLocation, SourceSpan, Span, ValueSource,
};
use typol_report::{DiagnosticReport as StructuredDiagnosticReport, ReportContext};
use typol_schema::{
    ResolverError, SchemaCompileErrors, SchemaCompileOptions, SchemaCompiler, SchemaResolveRequest,
    SchemaResolver, SchemaSource,
};
use typol_semantic::{CompiledSchema, SpanLocation};
use typol_std::{
    DiagnosticContext, DiagnosticRenderError, DiagnosticRenderer, DiagnosticReport,
    DiagnosticReportStage, DiagnosticReportTitle, DisplayError, DisplayFailurePoint,
    FileConfigInput, FileConfigInputLimits, FileSchemaCompileFailure, FileSchemaCompileOptions,
    FileSchemaCompiler, HostError, HostErrorKind, HostOperation, RenderOptions,
    SourceNameEditOptions, SourceNameLimits, SourceNames,
};

static LOWER_ERROR_DESCRIPTOR: DiagnosticContextDescriptor =
    DiagnosticContextDescriptor::new("typol-core", "test-budget", "retained-source-failure");
static LOWER_ERROR_CONTEXT: CoreDiagnosticContext =
    CoreDiagnosticContext::new(&LOWER_ERROR_DESCRIPTOR);

static UNAVAILABLE_TEST_EXCERPTS: SourceExcerptSet = SourceExcerptSet::unavailable();

struct NoImports;

impl SchemaResolver for NoImports {
    fn resolve<'src, 'slot, 'borrow>(
        &'src mut self,
        _: SchemaResolveRequest<'_>,
        out: Out<'slot, 'borrow, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'src>> {
        Err(out.fail(ResolverError::NotFound))
    }
}

struct NoEnvironment {
    calls: Rc<Cell<usize>>,
}

struct InvalidScalarEnvironment;

impl Environment for NoEnvironment {
    fn read<'env, 'slot, 'borrow>(
        &'env mut self,
        _: &str,
        _: EnvironmentReadContext<'_>,
        out: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        self.calls.set(self.calls.get() + 1);
        Err(out.fail(EnvironmentError::HostFailure))
    }
}

impl Environment for InvalidScalarEnvironment {
    fn read<'env, 'slot, 'borrow>(
        &'env mut self,
        _: &str,
        _: EnvironmentReadContext<'_>,
        _: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        Ok(Some(b"not-a-number"))
    }
}

fn compile(text: &str) -> Result<CompiledSchema, Box<dyn Error>> {
    let compiler = TestOpeo::call(|out| SchemaCompiler::new(SchemaCompileOptions::default(), out))
        .map_err(|error| io::Error::other(format!("Schema initialization failed: {error:?}")))?;
    TestOpeo::call(|out| {
        compiler.compile(
            SchemaSource::from_str(b"root", text),
            &mut NoImports,
            None,
            out,
        )
    })
    .map_err(|error| io::Error::other(format!("Schema compilation failed: {error:?}")).into())
}

fn render(
    diagnostics: &[Diagnostic],
    options: RenderOptions,
) -> Result<String, DiagnosticRenderError> {
    render_in(
        diagnostics,
        DiagnosticContext {
            source_name: &|_| None,
            span_location: &|_| None,
            excerpts: &UNAVAILABLE_TEST_EXCERPTS,
        },
        options,
    )
}

fn render_in(
    diagnostics: &[Diagnostic],
    context: DiagnosticContext<'_>,
    options: RenderOptions,
) -> Result<String, DiagnosticRenderError> {
    TestOpeo::call(|out| {
        DiagnosticRenderer::render(
            diagnostics,
            context,
            options,
            Translator::new(Language::English),
            out,
        )
    })
}

fn render_config_errors(
    errors: &ConfigParseErrors,
    language: Language,
) -> Result<String, DiagnosticRenderError> {
    let source_name = |source_id| {
        (errors.config_source_id() == Some(source_id))
            .then(|| errors.config_source_name())
            .flatten()
    };
    let span_location = |span| errors.span_location(span);
    TestOpeo::call(|out| {
        DiagnosticRenderer::render(
            errors.diagnostics(),
            DiagnosticContext {
                source_name: &source_name,
                span_location: &span_location,
                excerpts: errors.excerpts(),
            },
            RenderOptions::default(),
            Translator::new(language),
            out,
        )
    })
}

#[test]
fn layout_preserves_codes_labels_paths_provenance_and_optional_fields() -> Result<(), Box<dyn Error>>
{
    let primary = SourceSpan::new(
        SourceId::new(2),
        TestOpeo::call(|out| Span::new(0, 2, out))
            .map_err(|error| io::Error::other(format!("{error:?}")))?,
    );
    let label = SourceSpan::new(
        SourceId::new(7),
        TestOpeo::call(|out| Span::new(8, 10, out))
            .map_err(|error| io::Error::other(format!("{error:?}")))?,
    );
    let mut diagnostic = TestOpeo::call(|out| {
        Diagnostic::try_new(
            DiagnosticCode::CONFIG_VALIDATION_ERROR,
            "rule failed",
            None,
            out,
        )
    })?;
    diagnostic.set_primary(Some(primary));
    let mut path = TestOpeo::call(|out| InstancePath::new(None, out))?;
    TestOpeo::call(|out| path.push_field("items", out))?;
    TestOpeo::call(|out| path.push_index(1, out))?;
    TestOpeo::call(|out| path.push_map_key("a\"\\\n", out))?;
    TestOpeo::call(|out| path.push_payload("Some", out))?;
    TestOpeo::call(|out| diagnostic.try_set_path(Some(path), None, out))?;
    diagnostic.set_value_source(Some(ValueSource::ExplicitFallback));
    TestOpeo::call(|out| {
        diagnostic.try_add_label_with_text(
            DiagnosticLabelInput {
                span: label,
                text: "definition",
                role: RelatedLocationRole::AssociatedInput,
            },
            None,
            out,
        )
    })?;
    TestOpeo::call(|out| {
        diagnostic.try_add_label_with_text(
            DiagnosticLabelInput {
                span: primary,
                text: "another rule",
                role: RelatedLocationRole::AssociatedInput,
            },
            None,
            out,
        )
    })?;
    let diagnostics = [
        diagnostic,
        TestOpeo::call(|out| {
            Diagnostic::try_new(
                DiagnosticCode::ALLOCATION_FAILED,
                "allocation failed",
                None,
                out,
            )
        })?,
    ];
    let text = render_in(
        &diagnostics,
        DiagnosticContext {
            source_name: &|id| (id == primary.source_id()).then_some("config"),
            span_location: &|span| {
                (span == primary || span == label).then_some(SpanLocation {
                    start: SourceLocation::new(1, 2)?,
                    end: SourceLocation::new(3, 4)?,
                })
            },
            excerpts: &UNAVAILABLE_TEST_EXCERPTS,
        },
        RenderOptions::default(),
    )?;
    ensure_eq!(
        text,
        concat!(
            "CONFIG_VALIDATION_ERROR (12): rule failed",
            "\nKind: CONFIG_VALIDATION_ERROR",
            "\nDiagnostic stage: constraint_validation",
            "\nPath: $.items[1][\"a\\\"\\\\\\n\"]::Some",
            "\nLocation: config:1:2-3:4\n  = source excerpt unavailable",
            "\nRelated location (associated input): source#7:1:2-3:4: definition\n  = source excerpt unavailable",
            "\nRelated location (associated input): config:1:2-3:4: another rule\n  = source excerpt unavailable",
            "\nSource: ExplicitFallback",
            "\n\nALLOCATION_FAILED (2): allocation failed",
            "\nKind: ALLOCATION_FAILED",
            "\nDiagnostic stage: internal"
        )
    );
    ensure_eq!(
        diagnostics
            .first()
            .ok_or_else(|| io::Error::other("diagnostic is missing"))?
            .code(),
        DiagnosticCode::CONFIG_VALIDATION_ERROR
    );
    ensure_eq!(render(&[], RenderOptions::default())?, "");

    Ok(())
}

#[test]
fn localized_parser_builtin_errors_use_catalog_keys() -> Result<(), Box<dyn Error>> {
    let schema = compile("entry struct Root {}")?;
    let mut environment = NoEnvironment {
        calls: Rc::new(Cell::new(0)),
    };
    let undeclared = TestOpeo::call(|out| {
        ConfigParser::new(&schema).parse(
            ConfigParseRequest::new(
                &mut SliceSource::new(b"{missing: 1}"),
                "config",
                ConfigParseOptions::default().with_translator(Translator::new(Language::Chinese)),
            ),
            &mut environment,
            out,
        )
    })
    .err()
    .ok_or_else(|| io::Error::other("expected undeclared-field parsing to fail"))?;
    let text = render_config_errors(&undeclared, Language::Chinese)?;
    ensure!(text.contains("对象包含未声明的字段"));
    ensure!(!text.contains("object contains an undeclared field"));

    let schema = compile(r#"entry struct Root { value: u8 @env("PORT") }"#)?;
    let mut environment = InvalidScalarEnvironment;
    let invalid_environment = TestOpeo::call(|out| {
        ConfigParser::new(&schema).parse(
            ConfigParseRequest::new(
                &mut SliceSource::new(br#"{value: env("PORT")}"#),
                "config",
                ConfigParseOptions::default().with_translator(Translator::new(Language::Chinese)),
            ),
            &mut environment,
            out,
        )
    })
    .err()
    .ok_or_else(|| io::Error::other("expected invalid environment scalar to fail"))?;
    let text = render_config_errors(&invalid_environment, Language::Chinese)?;
    ensure!(text.contains("环境变量文本与目标标量类型或取值范围不兼容"));
    ensure!(!text.contains("environment text is incompatible"));
    Ok(())
}

#[test]
fn public_error_displays_keep_the_generated_message() -> Result<(), Box<dyn Error>> {
    let translator = Translator::new(Language::English);
    let diagnostic = TestOpeo::call(|out| {
        Diagnostic::try_new(
            DiagnosticCode::HOST_ADAPTER_INVARIANT_FAILURE,
            t! { name: "diagnostic.emergency.host_adapter_invariant_failure", locale: translator },
            None,
            out,
        )
    })?;
    let parse_errors = ConfigParseErrors::from(diagnostic);
    let rendered = render(parse_errors.diagnostics(), RenderOptions::default())?;
    ensure!(rendered.contains("host adapter invariant failed"));

    let value_errors = ConfigValueErrors::from(parse_errors);
    let mut value_text = String::new();
    ensure!(std::fmt::write(&mut value_text, format_args!("{value_errors}")).is_err());
    ensure!(value_text.is_empty());

    let schema_errors = SchemaCompileErrors::budget_failure(
        BudgetError::ResourceLimit(ResourceLimit::WorkspaceBytes),
        translator,
    );
    let schema_text = schema_errors.to_string();
    ensure!(schema_text.contains("workspace byte limit exceeded"));
    ensure!(!schema_text.contains("diagnostic message unavailable"));
    Ok(())
}

#[test]
fn localized_report_translates_builtin_text_and_preserves_custom_text() -> Result<(), Box<dyn Error>>
{
    let span = SourceSpan::new(
        SourceId::new(2),
        TestOpeo::call(|out| Span::new(0, 1, out))
            .map_err(|error| io::Error::other(format!("{error:?}")))?,
    );
    let mut diagnostic = TestOpeo::call(|out| {
        Diagnostic::try_new(
            DiagnosticCode::SCHEMA_SEMANTIC_ERROR,
            t! {
                name: "diagnostic.schema.root_missing_entry",
                locale: Translator::new(Language::English)
            },
            None,
            out,
        )
    })?;
    diagnostic.set_primary(Some(span));
    let label = t! {
        name: "diagnostic.label.constraint_defined_here",
        locale: Translator::new(Language::English)
    }
    .map_err(io::Error::other)?;
    TestOpeo::call(|out| {
        diagnostic.try_add_label_with_text(
            DiagnosticLabelInput {
                span,
                text: label,
                role: RelatedLocationRole::ConstraintDefinition,
            },
            None,
            out,
        )
    })?;
    let location = SpanLocation {
        start: SourceLocation::new(1, 1).ok_or_else(|| io::Error::other("invalid location"))?,
        end: SourceLocation::new(1, 2).ok_or_else(|| io::Error::other("invalid location"))?,
    };
    let translated = TestOpeo::call(|out| {
        DiagnosticRenderer::render_report(
            DiagnosticReport {
                title: DiagnosticReportTitle::OperationFailed,
                stage: Some(DiagnosticReportStage::ConfigurationReading),
                diagnostics: from_ref(&diagnostic).iter(),
                diagnostic_count: 1,
                context: DiagnosticContext {
                    source_name: &|_| Some("config"),
                    span_location: &|_| Some(location),
                    excerpts: &UNAVAILABLE_TEST_EXCERPTS,
                },
                options: RenderOptions::default(),
                translator: Translator::new(Language::Chinese),
            },
            out,
        )
    })?;

    ensure!(translated.contains("Typol 操作失败"));
    ensure!(translated.contains("读取配置"));
    ensure!(translated.contains("1 条诊断"));
    ensure!(translated.contains("root schema must declare an entry struct"));
    ensure!(translated.contains("位置: config:1:1-1:2"));
    ensure!(translated.contains("诊断类型: SCHEMA_SEMANTIC_ERROR"));
    ensure!(translated.contains("诊断阶段: schema_compilation"));
    ensure!(translated.contains("相关位置 (约束定义): config:1:1-1:2: constraint defined here"));

    let custom = TestOpeo::call(|out| {
        Diagnostic::try_new(
            DiagnosticCode::CONFIG_VALIDATION_ERROR,
            "custom assertion: account is not enabled",
            None,
            out,
        )
    })?;
    let custom_text = TestOpeo::call(|out| {
        DiagnosticRenderer::render(
            from_ref(&custom),
            DiagnosticContext {
                source_name: &|_| None,
                span_location: &|_| None,
                excerpts: &UNAVAILABLE_TEST_EXCERPTS,
            },
            RenderOptions::default(),
            Translator::new(Language::Chinese),
            out,
        )
    })?;
    ensure!(custom_text.contains("custom assertion: account is not enabled"));

    let custom_resource = TestOpeo::call(|out| {
        Diagnostic::try_new(
            DiagnosticCode::SOURCE_BYTES_LIMIT_EXCEEDED,
            "custom resource detail",
            None,
            out,
        )
    })?;
    let custom_resource_text = TestOpeo::call(|out| {
        DiagnosticRenderer::render(
            from_ref(&custom_resource),
            DiagnosticContext {
                source_name: &|_| None,
                span_location: &|_| None,
                excerpts: &UNAVAILABLE_TEST_EXCERPTS,
            },
            RenderOptions::default(),
            Translator::new(Language::Chinese),
            out,
        )
    })?;
    ensure!(custom_resource_text.contains("custom resource detail"));

    let mut invalid = Diagnostic::failure(
        DiagnosticCode::CONFIG_VALUE_ERROR,
        DiagnosticFailure::Budget(BudgetError::ResourceLimit(ResourceLimit::WorkspaceBytes)),
    );
    invalid.set_primary(Some(span));
    let failure = TestOpeo::call(|out| {
        DiagnosticRenderer::render(
            from_ref(&invalid),
            DiagnosticContext {
                source_name: &|_| None,
                span_location: &|_| None,
                excerpts: &UNAVAILABLE_TEST_EXCERPTS,
            },
            RenderOptions::default(),
            Translator::new(Language::Chinese),
            out,
        )
    });
    ensure!(matches!(
        failure,
        Err(DiagnosticRenderError::Diagnostic(
            DiagnosticFailure::Budget(BudgetError::ResourceLimit(ResourceLimit::WorkspaceBytes))
        ))
    ));

    let emergency = Diagnostic::emergency(
        DiagnosticCode::SOURCE_BYTES_LIMIT_EXCEEDED,
        "resource limit exceeded",
    );
    let localized_emergency = TestOpeo::call(|out| {
        DiagnosticRenderer::render(
            from_ref(&emergency),
            DiagnosticContext {
                source_name: &|_| None,
                span_location: &|_| None,
                excerpts: &UNAVAILABLE_TEST_EXCERPTS,
            },
            RenderOptions::default(),
            Translator::new(Language::Chinese),
            out,
        )
    })?;
    ensure!(localized_emergency.contains("resource limit exceeded"));
    Ok(())
}

#[test]
fn arbitrary_path_names_are_unambiguous_in_both_renderers() -> Result<(), Box<dyn Error>> {
    let mut diagnostic = TestOpeo::call(|out| {
        Diagnostic::try_new(
            DiagnosticCode::CONFIG_VALIDATION_ERROR,
            "path test",
            None,
            out,
        )
    })?;
    let mut path = TestOpeo::call(|out| InstancePath::new(None, out))?;
    TestOpeo::call(|out| path.push_field("a.b", out))?;
    TestOpeo::call(|out| path.push_field("a", out))?;
    TestOpeo::call(|out| path.push_field("b", out))?;
    TestOpeo::call(|out| path.push_field("struct", out))?;
    TestOpeo::call(|out| path.push_field("arr[0]", out))?;
    TestOpeo::call(|out| path.push_field("x\"y", out))?;
    TestOpeo::call(|out| path.push_field("line\nbreak", out))?;
    TestOpeo::call(|out| path.push_map_key("a.b", out))?;
    TestOpeo::call(|out| path.push_payload("Variant", out))?;
    TestOpeo::call(|out| path.push_payload("V::x", out))?;
    TestOpeo::call(|out| diagnostic.try_set_path(Some(path), None, out))?;

    let expected_path = concat!(
        "$.\"a.b\".a.b",
        ".\"struct\"",
        ".\"arr[0]\"",
        ".\"x\\\"y\"",
        ".\"line\\nbreak\"",
        "[\"a.b\"]",
        "::Variant",
        "::\"V::x\""
    );
    let rendered = render(from_ref(&diagnostic), RenderOptions::default())?;
    ensure!(rendered.contains(&format!("Path: {expected_path}")));

    let source_names = |_: SourceId| None;
    let span_locations = |_: SourceSpan| None;
    let excerpts = SourceExcerptSet::unavailable();
    let context = ReportContext::new(&source_names, &span_locations, &excerpts);
    let report = StructuredDiagnosticReport::new(
        "",
        None,
        from_ref(&diagnostic),
        context,
        Translator::new(Language::English),
    );
    let mut report_text = String::new();
    report.write_to(&mut report_text)?;
    ensure!(report_text.contains(&format!("Path: {expected_path}")));

    Ok(())
}

#[test]
fn every_dynamic_channel_escapes_controls_and_bidi_without_double_escaping()
-> Result<(), Box<dyn Error>> {
    let raw =
        "Ḁḁ\\\n\r\t\0\u{1b}\u{7f}\u{85}\u{9f}\u{2028}\u{2029}\u{202a}\u{202e}\u{2066}\u{2069}";
    let escaped = "Ḁḁ\\\\\\n\\r\\t\\u{0}\\u{1b}\\u{7f}\\u{85}\\u{9f}\\u{2028}\\u{2029}\\u{202a}\\u{202e}\\u{2066}\\u{2069}";
    let span = SourceSpan::new(
        SourceId::new(0),
        TestOpeo::call(|out| Span::new(0, 0, out))
            .map_err(|error| io::Error::other(format!("{error:?}")))?,
    );
    let mut diagnostic = TestOpeo::call(|out| {
        Diagnostic::try_new(DiagnosticCode::CONFIG_VALUE_ERROR, raw, None, out)
    })?;
    diagnostic.set_primary(Some(span));
    TestOpeo::call(|out| {
        diagnostic.try_add_label_with_text(
            DiagnosticLabelInput {
                span,
                text: raw,
                role: RelatedLocationRole::AssociatedInput,
            },
            None,
            out,
        )
    })?;
    let mut path = TestOpeo::call(|out| InstancePath::new(None, out))?;
    TestOpeo::call(|out| path.push_field(raw, out))?;
    TestOpeo::call(|out| path.push_map_key(raw, out))?;
    TestOpeo::call(|out| path.push_payload(raw, out))?;
    TestOpeo::call(|out| diagnostic.try_set_path(Some(path), None, out))?;
    let text = render_in(
        &[diagnostic],
        DiagnosticContext {
            source_name: &|_| Some(raw),
            span_location: &|_| {
                Some(SpanLocation {
                    start: SourceLocation::new(1, 1)?,
                    end: SourceLocation::new(1, 1)?,
                })
            },
            excerpts: &UNAVAILABLE_TEST_EXCERPTS,
        },
        RenderOptions::default(),
    )?;
    ensure_eq!(text.matches(escaped).count(), 7);
    ensure!(text.chars().all(|c| c == '\n' || !c.is_control()));
    for code in (0..=31)
        .chain(127..=159)
        .chain(0x2028..=0x202e)
        .chain(0x2066..=0x2069)
    {
        let message = char::from_u32(code)
            .ok_or_else(|| io::Error::other("invalid Unicode code point"))?
            .to_string();
        let diagnostic = TestOpeo::call(|out| {
            Diagnostic::try_new(DiagnosticCode::CONFIG_VALUE_ERROR, &message, None, out)
        })?;
        let text = render(&[diagnostic], RenderOptions::default())?;
        let character =
            char::from_u32(code).ok_or_else(|| io::Error::other("invalid Unicode code point"))?;
        if character == '\n' {
            ensure_eq!(text.matches('\n').count(), 2);
        } else {
            ensure!(!text.contains(character));
        }
    }

    Ok(())
}

#[test]
fn unavailable_snapshots_do_not_invent_names_or_positions() -> Result<(), Box<dyn Error>> {
    let mut diagnostic = Diagnostic::emergency(
        DiagnosticCode::HOST_ADAPTER_INVARIANT_FAILURE,
        "internal error",
    );
    let span = SourceSpan::new(
        SourceId::new(0),
        TestOpeo::call(|out| Span::new(0, 0, out))
            .map_err(|error| io::Error::other(format!("{error:?}")))?,
    );
    diagnostic.set_primary(Some(span));
    TestOpeo::call(|out| {
        diagnostic.try_add_label_with_text(
            DiagnosticLabelInput {
                span,
                text: "rule",
                role: RelatedLocationRole::AssociatedInput,
            },
            None,
            out,
        )
    })?;
    let text = render(&[diagnostic], RenderOptions::default())?;
    ensure_eq!(
        text,
        concat!(
            "HOST_ADAPTER_INVARIANT_FAILURE (1024): internal error",
            "\nKind: HOST_ADAPTER_INVARIANT_FAILURE",
            "\nDiagnostic stage: host_io",
            "\nLocation: location unavailable\n  = source excerpt unavailable",
            "\nRelated location (associated input): location unavailable: rule\n  = source excerpt unavailable"
        )
    );
    let error = SchemaCompileErrors::budget_failure(
        BudgetError::Allocation,
        Translator::new(Language::English),
    );
    ensure_eq!(
        render(
            from_ref(
                error
                    .diagnostics()
                    .first()
                    .ok_or_else(|| io::Error::other("diagnostic is missing"))?,
            ),
            RenderOptions::default(),
        )?,
        concat!(
            "ALLOCATION_FAILED (2): memory allocation failed",
            "\nKind: ALLOCATION_FAILED",
            "\nDiagnostic stage: internal"
        )
    );

    Ok(())
}

#[test]
fn exact_output_and_execution_limits_are_atomic_and_leave_business_errors_intact()
-> Result<(), Box<dyn Error>> {
    let message = "Ḁḁ\n".repeat(200);
    let diagnostic = TestOpeo::call(|out| {
        Diagnostic::try_new(DiagnosticCode::CONFIG_VALUE_ERROR, &message, None, out)
    })?;
    let diagnostics = [diagnostic];
    let text = render(&diagnostics, RenderOptions::default())?;
    let options = RenderOptions::new(RenderOptions::default().execution(), text.len());
    ensure_eq!(render(&diagnostics, options)?, text);
    ensure_eq!(
        render(
            &diagnostics,
            RenderOptions::new(options.execution(), text.len() - 1)
        ),
        Err(DiagnosticRenderError::Display(DisplayError::ResourceLimit(
            ResourceLimit::ConfigFormatOutputBytes
        )))
    );
    for (limits, limit) in [
        (ExecutionLimits::new(0, 8192), ResourceLimit::ExecutionSteps),
        (
            ExecutionLimits::new(8192, 0),
            ResourceLimit::FormattingWorkspaceBytes,
        ),
        (
            ExecutionLimits::new(usize::MAX, 1),
            ResourceLimit::FormattingWorkspaceBytes,
        ),
    ] {
        ensure_eq!(
            render(&diagnostics, RenderOptions::new(limits, usize::MAX)),
            Err(DiagnosticRenderError::Display(DisplayError::ResourceLimit(
                limit
            )))
        );
    }
    let minimum = (1..20_000)
        .find(|steps| {
            render(
                &diagnostics,
                RenderOptions::new(ExecutionLimits::new(*steps, 16_384), text.len()),
            )
            .is_ok()
        })
        .ok_or_else(|| {
            io::Error::other("could not find a render step count that fits the budget")
        })?;
    ensure_eq!(
        render(
            &diagnostics,
            RenderOptions::new(ExecutionLimits::new(minimum - 1, 16_384), text.len())
        ),
        Err(DiagnosticRenderError::Display(DisplayError::ResourceLimit(
            ResourceLimit::ExecutionSteps
        )))
    );
    let diagnostic = diagnostics
        .first()
        .ok_or_else(|| io::Error::other("diagnostic is missing"))?;
    ensure_eq!(diagnostic.code(), DiagnosticCode::CONFIG_VALUE_ERROR);
    ensure_eq!(
        diagnostic.message().map_err(io::Error::other)?,
        "Ḁḁ\n".repeat(200)
    );

    Ok(())
}

#[test]
fn localized_rendering_enforces_output_and_work_limits() -> Result<(), Box<dyn Error>> {
    let diagnostic = TestOpeo::call(|out| {
        Diagnostic::try_new(
            DiagnosticCode::CONFIG_VALUE_ERROR,
            "bounded direct write",
            None,
            out,
        )
    })?;
    let diagnostics = [diagnostic];
    let expected = render(&diagnostics, RenderOptions::default())?;
    let exact = RenderOptions::new(RenderOptions::default().execution(), expected.len());
    ensure_eq!(render(&diagnostics, exact)?, expected);
    for max_output_bytes in [0, expected.len() - 1] {
        ensure_eq!(
            render(
                &diagnostics,
                RenderOptions::new(exact.execution(), max_output_bytes),
            ),
            Err(DiagnosticRenderError::Display(DisplayError::ResourceLimit(
                ResourceLimit::ConfigFormatOutputBytes
            )))
        );
    }

    let work_exhausted = RenderOptions::new(
        ExecutionLimits::new(0, exact.execution().max_workspace_bytes()),
        expected.len(),
    );
    ensure_eq!(
        render(&diagnostics, work_exhausted),
        Err(DiagnosticRenderError::Display(DisplayError::ResourceLimit(
            ResourceLimit::ExecutionSteps
        )))
    );

    let host = HostError::budget(
        HostOperation::ConfigRead,
        BudgetError::ResourceLimit(ResourceLimit::SourceBytes),
    );
    let rendered_host = TestOpeo::call(|out| {
        DiagnosticRenderer::render_host(
            &host,
            RenderOptions::new(RenderOptions::default().execution(), usize::MAX),
            Translator::new(Language::English),
            out,
        )
    })?;
    ensure!(rendered_host.contains("source byte limit exceeded"));

    Ok(())
}

#[test]
fn config_two_source_snapshots_survive_inputs_and_schema_and_keep_unicode_crlf()
-> Result<(), Box<dyn Error>> {
    let environment_calls = Rc::new(Cell::new(0));
    let mut environment = NoEnvironment {
        calls: Rc::clone(&environment_calls),
    };
    let errors = {
        let schema = compile("entry struct Root {\r\n value: string @length(4, 8)\r\n}")?;
        let text = String::from("{\r\n value: \"Ḁ\"\r\n}");
        TestOpeo::call(|out| {
            ConfigParser::new(&schema).parse(
                ConfigParseRequest::new(
                    &mut SliceSource::new(text.as_bytes()),
                    "config\npublic",
                    ConfigParseOptions::default(),
                ),
                &mut environment,
                out,
            )
        })
        .err()
        .ok_or_else(|| io::Error::other("expected operation to fail"))?
    };
    let diagnostic = errors
        .diagnostics()
        .first()
        .ok_or_else(|| io::Error::other("diagnostic is missing"))?;
    let primary = diagnostic
        .primary()
        .ok_or_else(|| io::Error::other("primary location is missing"))?;
    let label = diagnostic
        .labels()
        .first()
        .ok_or_else(|| io::Error::other("label is missing"))?
        .span();
    ensure_ne!(primary.source_id(), label.source_id());
    let location = errors
        .span_location(primary)
        .ok_or_else(|| io::Error::other("primary location snapshot is missing"))?;
    ensure_eq!(location.start.line(), 2);
    ensure_eq!(location.start.column(), 9);
    ensure_eq!(location.end.column(), 12);
    let text = render_in(
        errors.diagnostics(),
        DiagnosticContext {
            source_name: &|id| {
                if Some(id) == errors.config_source_id() {
                    errors.config_source_name()
                } else {
                    Some("Schema")
                }
            },
            span_location: &|span| errors.span_location(span),
            excerpts: errors.excerpts(),
        },
        RenderOptions::default(),
    )?;
    ensure!(text.contains("Location: config\\npublic:2:9-2:12"));
    ensure!(text.contains("Related location (constraint definition): Schema:2:"));
    ensure!(text.contains("| 2 |  value: \"Ḁ\""));
    ensure!(text.contains("\n  |           ^^^"));
    let schema = compile("entry struct Root {}")?;
    let eof = TestOpeo::call(|out| {
        ConfigParser::new(&schema).parse(
            ConfigParseRequest::new(
                &mut SliceSource::new(("{\r\n").as_bytes()),
                "EOF",
                ConfigParseOptions::default(),
            ),
            &mut environment,
            out,
        )
    })
    .err()
    .ok_or_else(|| io::Error::other("expected operation to fail"))?;
    let span = eof
        .diagnostics()
        .first()
        .ok_or_else(|| io::Error::other("diagnostic is missing"))?
        .primary()
        .ok_or_else(|| io::Error::other("primary location is missing"))?;
    ensure!(span.span().is_empty());
    let location = eof
        .span_location(span)
        .ok_or_else(|| io::Error::other("EOF location snapshot is missing"))?;
    let expected = SourceLocation::new(2, 1)
        .ok_or_else(|| io::Error::other("expected location is invalid"))?;
    ensure_eq!(location.start, expected);
    ensure_eq!(environment_calls.get(), 0);

    Ok(())
}

fn schema_error(
    text: &str,
    name: &str,
) -> Result<(SchemaCompileErrors, SourceNames), Box<dyn Error>> {
    let compiler = TestOpeo::call(|out| SchemaCompiler::new(SchemaCompileOptions::default(), out))
        .map_err(|error| io::Error::other(format!("{error:?}")))?;
    let context = TestOpeo::call(|out| compiler.context(out))
        .map_err(|error| io::Error::other(format!("{error:?}")))?;
    let mut names =
        TestOpeo::call(|out| SourceNames::new(SourceNameLimits::default(), &context, out))
            .map_err(|error| io::Error::other(format!("{error:?}")))?;
    let errors = TestOpeo::call(|out| {
        compiler.compile(
            SchemaSource::from_str(b"root", text),
            &mut NoImports,
            Some(&mut names),
            out,
        )
    })
    .err()
    .ok_or_else(|| io::Error::other("expected operation to fail"))?;
    TestOpeo::call(|out| {
        names.set_name(
            SourceId::new(0),
            name,
            SourceNameEditOptions::default(),
            out,
        )
    })
    .map_err(|error| io::Error::other(format!("{error:?}")))?;
    Ok((errors, names))
}

#[test]
fn same_source_id_in_different_sessions_uses_each_errors_owned_context()
-> Result<(), Box<dyn Error>> {
    let first = schema_error("entry struct Root { value: Missing }", "first\u{1b}")?;
    let second = schema_error("\r\nentry struct Root { value: Missing }", "second")?;
    for (errors, names) in [&first, &second] {
        let mut text = String::new();
        for (index, diagnostic) in errors.diagnostics().iter().enumerate() {
            if index > 0 {
                text.push_str("\n\n");
            }
            let rendered = render_in(
                from_ref(diagnostic),
                DiagnosticContext {
                    source_name: &|id| names.name(id),
                    span_location: &|span| errors.span_location(span),
                    excerpts: errors.excerpts(),
                },
                RenderOptions::default(),
            )?;
            text.push_str(&rendered);
        }
        if names
            .name(SourceId::new(0))
            .ok_or_else(|| io::Error::other("source name is missing"))?
            .starts_with("first")
        {
            ensure!(text.contains("first\\u{1b}:1:"));
            ensure!(!text.contains("second"));
        } else {
            ensure!(text.contains("second:2:"));
            ensure!(!text.contains("first"));
        }
    }

    Ok(())
}

#[test]
fn secret_map_keys_and_values_are_not_restored_by_rendering() -> Result<(), Box<dyn Error>> {
    let environment_calls = Rc::new(Cell::new(0));
    let mut environment = NoEnvironment {
        calls: Rc::clone(&environment_calls),
    };
    let schema = compile(
        "type Item = string @regex(\"^x$\"); entry struct Root { values: map<string, Item> @secret }",
    )?;
    let errors = TestOpeo::call(|out| {
        ConfigParser::new(&schema).parse(
            ConfigParseRequest::new(
                &mut SliceSource::new(
                    ("{values: {\"PRIVATE-KEY\": \"PRIVATE-VALUE\"}}").as_bytes(),
                ),
                "config",
                ConfigParseOptions::default(),
            ),
            &mut environment,
            out,
        )
    })
    .err()
    .ok_or_else(|| io::Error::other("expected operation to fail"))?;
    drop(schema);
    let text = render_in(
        errors.diagnostics(),
        DiagnosticContext {
            source_name: &|_| Some("public"),
            span_location: &|span| errors.span_location(span),
            excerpts: errors.excerpts(),
        },
        RenderOptions::default(),
    )?;
    ensure!(text.contains("value is hidden"));
    ensure!(text.contains("Path: $.values"));
    ensure!(!text.contains("PRIVATE"));
    ensure_eq!(environment_calls.get(), 0);

    Ok(())
}

#[test]
fn host_and_file_wrappers_expose_only_safe_standard_error_chains() -> Result<(), Box<dyn Error>> {
    let propagated = HostError::budget(
        HostOperation::SourceNames,
        BudgetError::invariant(&LOWER_ERROR_CONTEXT),
    );
    ensure!(matches!(
        propagated.kind(),
        HostErrorKind::Internal(InternalError::BudgetInvariantFailure(context))
            if context == LOWER_ERROR_CONTEXT
    ));
    ensure_eq!(propagated.context().failure_point(), "budget-invariant");
    ensure_eq!(
        propagated
            .source_context()
            .ok_or_else(|| io::Error::other("host wrapper lost its underlying context"))?
            .failure_point(),
        "retained-source-failure"
    );
    let display = DisplayError::from(BudgetError::invariant(&LOWER_ERROR_CONTEXT));
    ensure_eq!(
        display
            .source_context()
            .ok_or_else(|| io::Error::other("display wrapper lost its underlying context"))?
            .failure_point(),
        "retained-source-failure"
    );

    let path = Path::new("/PRIVATE-PATH\u{1b}\n/missing.tschem");
    let budget = TestOpeo::call(|out| {
        ExecutionBudget::new(ExecutionLimits::new(16_777_216, 16_777_216), out)
    })?;
    let host = TestOpeo::call(|out| {
        FileConfigInput::open(
            path,
            FileConfigInputLimits::default(),
            1_048_576,
            &budget,
            out,
        )
    })
    .err()
    .ok_or_else(|| io::Error::other("expected operation to fail"))?;
    ensure!(host.source().is_none());
    let rendered = TestOpeo::call(|out| {
        DiagnosticRenderer::render_host(
            &host,
            RenderOptions::default(),
            Translator::new(Language::English),
            out,
        )
    })?;
    ensure_eq!(rendered, host.to_string());
    ensure!(!rendered.contains("PRIVATE"));
    ensure!(!rendered.contains("Error "));
    ensure_eq!(
        TestOpeo::call(|out| {
            DiagnosticRenderer::render_host(
                &host,
                RenderOptions::new(RenderOptions::default().execution(), rendered.len() - 1),
                Translator::new(Language::English),
                out,
            )
        }),
        Err(DiagnosticRenderError::Display(DisplayError::ResourceLimit(
            ResourceLimit::ConfigFormatOutputBytes
        )))
    );
    let errors = TestOpeo::call(|out| {
        FileSchemaCompiler::compile(path, FileSchemaCompileOptions::default(), out)
    })
    .err()
    .ok_or_else(|| io::Error::other("expected operation to fail"))?;
    match errors.failure() {
        FileSchemaCompileFailure::Root(host) => {
            let wrapped = errors
                .host_failure()
                .ok_or_else(|| io::Error::other("wrapper did not preserve the host error"))?;
            ensure_eq!(wrapped.context(), host.context());
            ensure_eq!(wrapped.operation(), host.operation());
            ensure_eq!(wrapped.kind(), host.kind());
            ensure_eq!(
                errors
                    .source()
                    .ok_or_else(|| io::Error::other("file source error is missing"))?
                    .to_string(),
                host.to_string()
            )
        }
        FileSchemaCompileFailure::Compilation(_) => {
            return Err(io::Error::other("expected root read to fail").into());
        }
    }
    ensure!(!format!("{errors:?} {errors}").contains("PRIVATE"));
    let errors = TestOpeo::call(|out| {
        FileSchemaCompiler::compile(
            path,
            FileSchemaCompileOptions::new(
                SchemaCompileOptions::default().with_execution(ExecutionLimits::new(0, 0)),
                Default::default(),
                Default::default(),
            ),
            out,
        )
    })
    .err()
    .ok_or_else(|| io::Error::other("expected operation to fail"))?;
    ensure!(
        errors
            .source()
            .and_then(|source| source.downcast_ref::<SchemaCompileErrors>())
            .is_some()
    );
    for error in [
        DisplayError::AllocationFailed,
        DisplayError::ResourceLimit(ResourceLimit::SourceBytes),
        DisplayError::ShapeMismatch,
    ] {
        ensure!(error.source().is_none());
        ensure!(error.to_string().starts_with("Diagnostic rendering failed"));
    }
    let internal = DisplayError::internal(DisplayFailurePoint::SchemaAccess);
    let context = internal
        .context()
        .ok_or_else(|| io::Error::other("internal display error is missing its context"))?;
    ensure_eq!(context.subsystem(), "typol-std");
    ensure_eq!(context.stage(), "display");
    ensure_eq!(context.failure_point(), "schema-access");
    ensure_eq!(
        internal.to_string(),
        "Diagnostic rendering failed: invalid internal relationship"
    );

    Ok(())
}
