use core::{iter::once, slice::from_ref};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    ptr::null_mut,
    string::String,
    thread_local,
};
use typol_config::{
    ConfigDiagnosticRedactor, ConfigLimits, ConfigParseErrors, ConfigParseOptions,
    ConfigParseRequest, ConfigValidationOptions, ConfigValidationRequest, ConfigValueErrors,
    ConfigValueProcessor, Environment, EnvironmentError, EnvironmentReadContext,
    ValidatedConfigValue,
    opeo::{OResult, Out},
};
use typol_core::{
    AllocationError, BudgetError, ByteSource, ConfigValue, Diagnostic, DiagnosticCode,
    ExcerptLimits, ExcerptStatus, ExecutionBudget, ExecutionLimits, InstancePath, LineIndex,
    LocatedSpan, LocationError, SliceSource, SourceCapture, SourceExcerptSet, SourceId, SourceSpan,
    Span, SpanError, SpanLocation, ValueError,
};
use typol_policy::ResourceLimits;
use typol_schema::{SchemaCompileOptions, SchemaSource};
use typol_semantic::CompiledSchema;
use typol_test_support::*;
use typol_test_support::{ConfigTestSupport as Support, TestOpeoOutcome};

#[derive(Clone, Copy)]
struct TestLimits {
    resources: ResourceLimits,
    config: ConfigLimits,
}

impl From<&ResourceLimits> for TestLimits {
    fn from(resources: &ResourceLimits) -> Self {
        Self {
            resources: *resources,
            config: ConfigLimits::default(),
        }
    }
}

impl From<&ConfigLimits> for TestLimits {
    fn from(config: &ConfigLimits) -> Self {
        Self {
            resources: ResourceLimits::default(),
            config: *config,
        }
    }
}

impl From<&(&ResourceLimits, &ConfigLimits)> for TestLimits {
    fn from(limits: &(&ResourceLimits, &ConfigLimits)) -> Self {
        Self {
            resources: *limits.0,
            config: *limits.1,
        }
    }
}

impl TestLimits {
    fn resources(self) -> ResourceLimits {
        self.resources
    }

    fn config(self) -> ConfigLimits {
        self.config
    }
}

fn outcome_error<T, E>(outcome: &TestOpeoOutcome<T, E>) -> Option<&E> {
    match outcome {
        TestOpeoOutcome::Value(_) => None,
        TestOpeoOutcome::Error(error) => Some(error),
    }
}

struct NoEnvironment;

impl Environment for NoEnvironment {
    fn read<'env, 'slot, 'borrow>(
        &'env mut self,
        _: &str,
        _: EnvironmentReadContext<'_>,
        _: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        Ok(None)
    }
}

struct Provider {
    calls: usize,
    result: Result<Option<&'static [u8]>, EnvironmentError>,
}

struct ValueSlots {
    allocation_errors: TestSlot<AllocationError>,
    value_errors: TestSlot<ValueError>,
    validation_errors: TestSlot<ConfigValueErrors>,
}

impl ValueSlots {
    fn new() -> Self {
        Self {
            allocation_errors: TestSlot::default(),
            value_errors: TestSlot::default(),
            validation_errors: TestSlot::default(),
        }
    }

    fn field(&mut self, name: &str, value: ConfigValue) -> TestResult<FieldValue> {
        self.allocation_errors
            .access(|out| FieldValue::try_new_unmetered(name, value, out))
            .map_err(|caught| TestFailure::new(format!("failed to create field: {:?}", caught)))
    }

    fn map_entry(&mut self, key: &str, value: ConfigValue) -> TestResult<MapEntry> {
        self.allocation_errors
            .access(|out| MapEntry::try_new_unmetered(key, value, out))
            .map_err(|caught| TestFailure::new(format!("failed to create map entry: {:?}", caught)))
    }

    fn structure(&mut self, fields: Vec<FieldValue>) -> TestResult<ConfigValue> {
        self.value_errors
            .access(|out| ValueFactory::try_structure(fields).map_err(|error| out.fail(error)))
            .map_err(|caught| {
                TestFailure::new(format!("failed to create structure value: {:?}", caught))
            })
    }

    fn map(&mut self, entries: Vec<MapEntry>) -> TestResult<ConfigValue> {
        self.value_errors
            .access(|out| ValueFactory::try_map(entries).map_err(|error| out.fail(error)))
            .map_err(|caught| TestFailure::new(format!("failed to create map value: {:?}", caught)))
    }

    fn string(&mut self, value: &str) -> TestResult<ConfigValue> {
        self.value_errors
            .access(|out| ValueFactory::try_string(value).map_err(|error| out.fail(error)))
            .map_err(|caught| TestFailure::new(format!("failed to create string: {:?}", caught)))
    }

    fn validate(
        &mut self,
        schema: &CompiledSchema,
        value: ConfigValue,
        options: ConfigValidationOptions,
    ) -> TestOpeoOutcome<ValidatedConfigValue, ConfigValueErrors> {
        match self.validation_errors.access(|out| {
            ConfigValueProcessor::new(schema).validate(
                value,
                ConfigValidationRequest::new(options),
                out,
            )
        }) {
            Ok(value) => TestOpeoOutcome::Value(value),
            Err(caught) => TestOpeoOutcome::Error(caught),
        }
    }
}

impl Environment for Provider {
    fn read<'env, 'slot, 'borrow>(
        &'env mut self,
        _: &str,
        _: EnvironmentReadContext<'_>,
        out: Out<'slot, 'borrow, EnvironmentError>,
    ) -> OResult<'slot, Option<&'env [u8]>> {
        self.calls += 1;
        match self.result {
            Ok(value) => Ok(value),
            Err(error) => Err(out.fail(error)),
        }
    }
}

fn compile(text: &str) -> TestResult<CompiledSchema> {
    Support::compile_schema(
        SchemaCompileOptions::default(),
        SchemaSource::from_str(b"root", text),
        None,
        None,
    )
    .test_value()
}

fn conversion_diagnostic(budget: &ExecutionBudget, fields: &[&str]) -> TestResult<Diagnostic> {
    let mut errors = TestSlot::<BudgetError>::default();
    errors
        .access(|mut out| {
            let mut path = InstancePath::new(Some(budget), out.reborrow())?;
            for field in fields {
                path.push_field(field, out.reborrow())?;
            }
            let mut diagnostic = Diagnostic::try_new(
                DiagnosticCode::RUST_CONVERSION_ERROR,
                "conversion failure",
                Some(budget),
                out.reborrow(),
            )?;
            diagnostic.try_set_path(Some(path), Some(budget), out.reborrow())?;
            Ok(diagnostic)
        })
        .map_err(|caught| {
            TestFailure::new(format!("failed to create test diagnostic: {:?}", caught))
        })
}

fn redact_conversion(
    schema: &CompiledSchema,
    diagnostic: &mut Diagnostic,
) -> Result<(), Diagnostic> {
    let mut errors = TestSlot::<Diagnostic>::default();
    errors.access(|out| ConfigDiagnosticRedactor::new(schema).redact_conversion(diagnostic, out))
}

fn parse(
    schema: &CompiledSchema,
    source: &str,
    limits: impl Into<TestLimits>,
) -> TestOpeoOutcome<ConfigValue, ConfigParseErrors> {
    let options = session_options(limits);
    Support::parse(
        schema,
        ConfigParseRequest::new(
            &mut SliceSource::new(source.as_bytes()),
            "allocation.tconf",
            options,
        ),
        &mut NoEnvironment,
    )
}

fn parse_with_provider(
    schema: &CompiledSchema,
    source: &str,
    bytes: Option<&'static [u8]>,
    limits: impl Into<TestLimits>,
) -> TestOpeoOutcome<ConfigValue, ConfigParseErrors> {
    let mut provider = Provider {
        calls: 0,
        result: Ok(bytes),
    };
    Support::parse(
        schema,
        config_request(
            (&mut SliceSource::new(source.as_bytes()), "allocation.tconf"),
            limits,
        ),
        &mut provider,
    )
}

#[derive(Clone, Copy)]
struct AllocationState {
    active: bool,
    fail_at: Option<usize>,
    persistent: bool,
    attempts: usize,
    live_bytes: isize,
}

thread_local! {
    static STATE: Cell<AllocationState> = const { Cell::new(AllocationState {
        active: false, fail_at: None, persistent: false, attempts: 0, live_bytes: 0,
    }) };
}

struct CheckedAllocator;

fn should_fail() -> bool {
    STATE
        .try_with(|state| {
            let mut current = state.get();
            if !current.active {
                return false;
            }
            let fail = current.fail_at.is_some_and(|index| {
                index == current.attempts || (current.persistent && index < current.attempts)
            });
            current.attempts += 1;
            state.set(current);
            fail
        })
        .unwrap_or(false)
}

fn account(bytes: isize) {
    let _ = STATE.try_with(|state| {
        let mut current = state.get();
        if current.active {
            current.live_bytes += bytes;
            state.set(current);
        }
    });
}

unsafe impl GlobalAlloc for CheckedAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if should_fail() {
            return null_mut();
        }
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            account(layout.size() as isize);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        account(-(layout.size() as isize));
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if should_fail() {
            return null_mut();
        }
        let result = unsafe { System.realloc(pointer, layout, new_size) };
        if !result.is_null() {
            account(new_size as isize - layout.size() as isize);
        }
        result
    }
}

#[global_allocator]
static ALLOCATOR: CheckedAllocator = CheckedAllocator;

struct AllocationScope;

impl AllocationScope {
    fn start(fail_at: Option<usize>) -> Self {
        STATE.with(|state| {
            state.set(AllocationState {
                active: true,
                fail_at,
                persistent: false,
                attempts: 0,
                live_bytes: 0,
            })
        });
        Self
    }

    fn focus(&self, fail_at: Option<usize>, persistent: bool) {
        STATE.with(|state| {
            let mut current = state.get();
            current.fail_at = fail_at;
            current.persistent = persistent;
            current.attempts = 0;
            state.set(current);
        });
    }

    fn finish(self) -> AllocationState {
        STATE.with(Cell::get)
    }
}

impl Drop for AllocationScope {
    fn drop(&mut self) {
        STATE.with(|state| {
            let mut current = state.get();
            current.active = false;
            state.set(current);
        });
    }
}

#[test]
fn allocation_failures_are_fatal_and_release_all_temporary_values() -> TestResult<()> {
    let schema = compile(
        r#"
        struct Child { text: string = "default-text", }
        enum E { Unit, Single(string), Object { text: string, }, }
        entry struct Root {
            child: Child, dict: map<string, string>, array: [option<string>], variant: E,
            defaults: map<string, [string]> = { "key": ["default"] },
            port: u8 @env("PORT") = 1,
            optional: option<Child> = {},
            unit_default: E = .Unit,
            single_default: E = .Single("default-payload"),
            object_default: E = .Object { text: "default-object" },
        }
    "#,
    )?;
    let mut limits = ResourceLimits::default();
    limits.set_excerpts(ExcerptLimits::new(0, 0, 0, 0));
    for source in [
        r#"{ child: {}, dict: { "one": "first", "two": "second" }, array: ["text", none], variant: .Single("payload") }"#,
        r#"{ child: {}, dict: { "o\u{00E9}": "first\n", "two": "second" }, array: ["te\txt", none], variant: .Single("pay\nload") }"#,
        r#"{ child: {}, dict: { "one": "first", "two": "second" }, array: ["text", none], variant: .Object { text: "payload" } }"#,
        r#"{ child: {}, dict: { "one": 1, "two": 2 }, array: ["text", none], variant: .Single("payload") }"#,
        r#"{ child: {}, dict: { "one": "first" }, array: ["text"], variant: .Single("payload"), port: env("PORT", 9) }"#,
    ] {
        let scope = AllocationScope::start(None);
        drop(parse(&schema, source, &limits));
        let baseline = scope.finish();
        test_eq!(baseline.live_bytes, 0)?;
        for fail_at in 0..baseline.attempts {
            let scope = AllocationScope::start(Some(fail_at));
            let result = parse(&schema, source, &limits);
            let allocation_failed = match &result {
                TestOpeoOutcome::Value(_) => false,
                TestOpeoOutcome::Error(errors) => errors
                    .diagnostics()
                    .last()
                    .is_some_and(|error| error.code() == DiagnosticCode::ALLOCATION_FAILED),
            };
            drop(result);
            let state = scope.finish();
            test_assert!(allocation_failed)?;
            test_eq!(state.live_bytes, 0)?;
        }
    }

    Ok(())
}

#[test]
fn session_cache_labels_snapshots_and_name_failures_release_every_allocation() -> TestResult<()> {
    let schema = compile(
        r#"
        type A = u8 @min(8);
        entry struct Root {
            first: A @env("X"), second: A @env("X"),
            defaults: [u8] = [2, 3],
        }
    "#,
    )?;
    let mut limits = ResourceLimits::default();
    limits.set_excerpts(ExcerptLimits::new(0, 0, 0, 0));
    let source = "{ first: env(\"X\"), second: env(\"X\") }";
    let mut baseline_attempts = None;
    for fail_at in once(None).chain((0..usize::MAX).map(Some)) {
        if let Some(attempts) = baseline_attempts {
            if fail_at.is_some_and(|index| index >= attempts) {
                break;
            }
        }
        let scope = AllocationScope::start(fail_at);
        let mut provider = Provider {
            calls: 0,
            result: Ok(Some(b"7")),
        };
        let options = session_options(&limits);
        let result = Support::parse(
            &schema,
            ConfigParseRequest::new(
                &mut SliceSource::new((source).as_bytes()),
                "allocation.tconf",
                options,
            ),
            &mut provider,
        );
        let errors = result.test_error()?;
        let last_code = errors.diagnostics().last().test_value()?.code();
        drop(errors);
        let state = scope.finish();
        if fail_at.is_none() {
            baseline_attempts = Some(state.attempts);
        }
        test_eq!(state.live_bytes, 0)?;
        if fail_at.is_some() {
            test_eq!(last_code, DiagnosticCode::ALLOCATION_FAILED)?;
        }
    }

    Ok(())
}

#[test]
fn initialization_failure_returns_inline_errors_without_another_allocation() -> TestResult<()> {
    let schema = compile("entry struct Root {}")?;
    for budget in [0, 1, 4] {
        let mut limits = ConfigLimits::default();
        limits = limits.with_max_diagnostics(budget);
        let scope = AllocationScope::start(Some(0));
        let mut provider = Provider {
            calls: 0,
            result: Ok(None),
        };
        let options = session_options(&limits);
        let errors = match Support::parse(
            &schema,
            ConfigParseRequest::new(&mut SliceSource::new(("{}").as_bytes()), "test", options),
            &mut provider,
        ) {
            TestOpeoOutcome::Value(_) => {
                return Err(TestFailure::new("the first allocation must fail"));
            }
            TestOpeoOutcome::Error(errors) => errors,
        };
        test_eq!(errors.diagnostics().len(), 1)?;
        test_eq!(
            TestCheck::item(errors.diagnostics(), 0)?.code(),
            DiagnosticCode::ALLOCATION_FAILED
        )?;
        test_assert!(errors.config_source_name().is_none())?;
        drop(errors);
        let state = scope.finish();
        test_eq!(state.attempts, 1)?;
        test_eq!(state.live_bytes, 0)?;
    }

    Ok(())
}

#[test]
fn real_environment_conversion_and_fallback_allocations_fail_atomically() -> TestResult<()> {
    let schema =
        compile(r#"entry struct Root { a: option<string> @env("X"), b: string @env("X"), }"#)?;
    let source = r#"{ a: env("X", "fallback-a"), b: env("X", "fallback-b") }"#;
    let mut limits = ResourceLimits::default();
    limits.set_excerpts(ExcerptLimits::new(0, 0, 0, 0));
    for bytes in [None, Some(&b"set-value"[..]), Some(&b"\xff"[..])] {
        let mut baseline_attempts = None;
        for fail_at in once(None).chain((0..usize::MAX).map(Some)) {
            if let Some(attempts) = baseline_attempts {
                if fail_at.is_some_and(|index| index >= attempts) {
                    break;
                }
            }
            let scope = AllocationScope::start(fail_at);
            let parsed = parse_with_provider(&schema, source, bytes, &limits);
            let code = outcome_error(&parsed)
                .and_then(|errors| errors.diagnostics().last())
                .map(|error| error.code());
            drop(parsed);
            let state = scope.finish();
            if fail_at.is_none() {
                baseline_attempts = Some(state.attempts);
            } else {
                test_eq!(code, Some(DiagnosticCode::ALLOCATION_FAILED))?;
            }
            test_eq!(state.live_bytes, 0)?;
        }
    }

    Ok(())
}

#[test]
fn real_local_constraints_and_regex_failures_release_every_allocation() -> TestResult<()> {
    let schema = compile(
        r#"
        type A = string @regex("^a") @length(2, 6);
        type O = option<A> @length(3, 5);
        entry struct Root {
            first: O @env("X"), second: A @env("X"),
            defaults: map<string, [O]> = { "key": ["abc", none] },
        }
    "#,
    )?;
    let mut limits = ResourceLimits::default();
    limits.set_excerpts(ExcerptLimits::new(0, 0, 0, 0));
    for (source, bytes) in [
        (
            r#"{ first: env("X", "abc"), second: env("X", "abc") }"#,
            Some(&b"abcd"[..]),
        ),
        (
            r#"{ first: env("X", "abc"), second: env("X", "abc") }"#,
            None,
        ),
        (r#"{ first: env("X"), second: env("X") }"#, Some(&b"x"[..])),
        (
            r#"{ first: env("X", "x"), second: env("X", "abc") }"#,
            Some(&b"abc"[..]),
        ),
    ] {
        let mut baseline_attempts = None;
        for fail_at in once(None).chain((0..usize::MAX).map(Some)) {
            if let Some(attempts) = baseline_attempts {
                if fail_at.is_some_and(|index| index >= attempts) {
                    break;
                }
            }
            let scope = AllocationScope::start(fail_at);
            let parsed = parse_with_provider(&schema, source, bytes, &limits);
            let code = outcome_error(&parsed)
                .and_then(|errors| errors.diagnostics().last())
                .map(|error| error.code());
            drop(parsed);
            let state = scope.finish();
            if fail_at.is_none() {
                baseline_attempts = Some(state.attempts);
            } else {
                test_eq!(code, Some(DiagnosticCode::ALLOCATION_FAILED))?;
            }
            test_eq!(state.live_bytes, 0)?;
        }
    }

    Ok(())
}

#[test]
fn expression_work_stack_allocation_failure_is_structured() -> TestResult<()> {
    let schema = compile(
        "entry struct Root { values: [[i8]] } @assert(self.values.all(|v| v.sum() == 127 && v.any(|v| v == 127)), \"rule\")",
    )?;
    let mut limits = ResourceLimits::default();
    limits.set_excerpts(ExcerptLimits::new(0, 0, 0, 0));
    let scope = AllocationScope::start(Some(0));
    let result = parse(&schema, "{values: [[127, 1, -1], [127]]}", &limits);
    let state = scope.finish();
    let errors = result.test_error()?;
    test_eq!(
        errors.diagnostics().last().test_value()?.code(),
        DiagnosticCode::ALLOCATION_FAILED
    )?;
    test_eq!(state.attempts, 1)?;
    test_eq!(state.live_bytes, 0)?;

    Ok(())
}

#[test]
fn collection_and_full_tree_assertion_allocations_fail_atomically() -> TestResult<()> {
    let schema = compile(
        r#"
        struct Child { n: i8, } @assert(self.n > root.limit, "child")
        entry struct Root {
            a: [i8] @unique @all(item > 0, "positive") @sum(min = 0),
            dict: map<string, i8> @all(value > 0, "positive"),
            limit: i8,
            child: Child = { n: 1 },
        } @assert(self.limit < 0, "root")
    "#,
    )?;
    let mut limits = ResourceLimits::default();
    limits.set_excerpts(ExcerptLimits::new(0, 0, 0, 0));
    for source in [
        r#"{ a: [1, 1, -1], dict: { "z": -1, "a": -2 }, limit: 0 }"#,
        r#"{ a: [1, 2], dict: { "z": 1 }, limit: 3 }"#,
        r#"{ a: [1, 2], dict: { "z": 1 }, limit: -1 }"#,
    ] {
        let scope = AllocationScope::start(None);
        drop(parse(&schema, source, &limits));
        let baseline = scope.finish();
        test_eq!(baseline.live_bytes, 0)?;
        for fail_at in 0..baseline.attempts {
            let scope = AllocationScope::start(Some(fail_at));
            let result = parse(&schema, source, &limits);
            let code = outcome_error(&result)
                .and_then(|errors| errors.diagnostics().last())
                .map(|error| error.code());
            drop(result);
            let state = scope.finish();
            test_eq!(code, Some(DiagnosticCode::ALLOCATION_FAILED))?;
            test_eq!(state.live_bytes, 0)?;
        }
    }

    Ok(())
}

#[test]
fn secret_paths_labels_cache_defaults_and_error_storage_fail_atomically() -> TestResult<()> {
    let schema = compile(
        r#"
        type Secret = string @secret @length(3, 20);
        struct Child { n: i8, } @assert(self.n > root.limit, "child")
        entry struct Root {
            first: Secret @env("X"), second: Secret @env("X"),
            dict: map<string, [i8]> @secret @all(value.all(|n| n > 0), "positive"),
            defaults: map<string, Child> @secret = { "private-default": { n: 1 } },
            limit: i8,
        }
    "#,
    )?;
    let mut limits = ResourceLimits::default();
    limits.set_excerpts(ExcerptLimits::new(0, 0, 0, 0));
    for (source, bytes) in [
        (
            r#"{ first: env("X"), second: env("X"), dict: { "private-key": [-1] }, limit: 2 }"#,
            Some(&b"x"[..]),
        ),
        (
            r#"{ first: env("X"), second: env("X"), dict: { "private-key": [1] }, limit: 0 }"#,
            Some(&b"valid"[..]),
        ),
        (
            r#"{ first: env("X", "valid"), second: env("X", "valid"), dict: { "private-key": [-1] }, limit: 2 }"#,
            None,
        ),
    ] {
        let scope = AllocationScope::start(None);
        drop(parse_with_provider(&schema, source, bytes, &limits));
        let baseline = scope.finish();
        test_eq!(baseline.live_bytes, 0)?;
        for fail_at in 0..baseline.attempts {
            let scope = AllocationScope::start(Some(fail_at));
            let result = parse_with_provider(&schema, source, bytes, &limits);
            let errors = result.test_error()?;
            test_eq!(
                errors.diagnostics().last().test_value()?.code(),
                DiagnosticCode::ALLOCATION_FAILED
            )?;
            test_assert!(
                errors
                    .diagnostics()
                    .iter()
                    .all(|error| error.path().is_none())
            )?;
            drop(errors);
            let state = scope.finish();
            test_eq!(state.live_bytes, 0)?;
        }
    }

    Ok(())
}

#[test]
fn conversion_redaction_allocation_failure_discards_the_original_path() -> TestResult<()> {
    let schema = compile("struct Child { member: u8 } entry struct Root { value: Child @secret }")?;
    let mut budget_errors = TestSlot::<BudgetError>::default();
    let budget = budget_errors
        .access(|out| ExecutionBudget::new(ExecutionLimits::new(1_000_000, 1_048_576), out))
        .map_err(|caught| TestFailure::new(format!("failed to create budget: {:?}", caught)))?;
    let scope = AllocationScope::start(None);
    let mut diagnostic = conversion_diagnostic(&budget, &["value", "member"])?;
    scope.focus(None, false);
    let result = redact_conversion(&schema, &mut diagnostic);
    drop(diagnostic);
    let baseline = scope.finish();
    test_assert!(result.is_ok())?;
    test_assert!(baseline.attempts > 0)?;
    test_eq!(baseline.live_bytes, 0)?;

    for fail_at in 0..baseline.attempts {
        let scope = AllocationScope::start(None);
        let mut diagnostic = conversion_diagnostic(&budget, &["value", "member"])?;
        scope.focus(Some(fail_at), false);
        let result = redact_conversion(&schema, &mut diagnostic);
        let safe = result.err();
        let code = safe.as_ref().map(Diagnostic::code);
        let path_is_clear = safe.as_ref().is_some_and(|error| error.path().is_none());
        drop(safe);
        drop(diagnostic);
        let state = scope.finish();
        test_eq!(code, Some(DiagnosticCode::ALLOCATION_FAILED))?;
        test_assert!(path_is_clear)?;
        test_eq!(state.live_bytes, 0)?;
    }

    Ok(())
}

#[test]
fn persistent_allocation_failure_returns_safe_nonempty_owned_errors() -> TestResult<()> {
    let schema = compile(r#"entry struct Root { value: map<string, u8> @secret, }"#)?;
    let source = r#"{ value: { "private-key": 999 } }"#;
    let mut limits = ResourceLimits::default();
    limits.set_excerpts(ExcerptLimits::new(0, 0, 0, 0));
    let scope = AllocationScope::start(None);
    drop(parse(&schema, source, &limits));
    let baseline = scope.finish();
    for fail_at in 0..baseline.attempts {
        let scope = AllocationScope::start(Some(fail_at));
        STATE.with(|state| {
            let mut current = state.get();
            current.persistent = true;
            state.set(current);
        });
        let errors = parse(&schema, source, &limits).test_error()?;
        test_eq!(errors.diagnostics().len(), 1)?;
        let diagnostic = TestCheck::item(errors.diagnostics(), 0)?;
        test_eq!(diagnostic.code(), DiagnosticCode::ALLOCATION_FAILED)?;
        test_assert!(diagnostic.path().is_none())?;
        test_assert!(errors.config_source_name().is_none())?;
        if let Some(span) = diagnostic.primary() {
            test_assert!(errors.span_location(span).is_none())?;
        }
        drop(errors);
        let state = scope.finish();
        test_eq!(state.live_bytes, 0)?;
    }

    Ok(())
}

#[test]
fn public_entry_allocations_fail_atomically_including_source_name() -> TestResult<()> {
    let schema = compile(
        r#"entry struct Root {
            a: string @env("X") @length(3, 8), b: string @env("X"),
            values: [u8] @unique,
        } @assert(self.values.len() == 2, "root")"#,
    )?;
    let mut limits = ResourceLimits::default();
    limits.set_excerpts(ExcerptLimits::new(0, 0, 0, 0));
    for text in [
        r#"{ a: env("X"), b: env("X"), values: [1, 2] }"#,
        r#"{ a: env("X"), b: env("X"), values: [1] }"#,
    ] {
        let mut attempts = None;
        for fail_at in once(None).chain((0..usize::MAX).map(Some)) {
            if let Some(attempts) = attempts {
                if fail_at.is_some_and(|index| index >= attempts) {
                    break;
                }
            }
            let scope = AllocationScope::start(fail_at);
            let mut provider = Provider {
                calls: 0,
                result: Ok(Some(b"valid")),
            };
            let result = Support::parse(
                &schema,
                config_request(
                    (&mut SliceSource::new(text.as_bytes()), "public-entry.tconf"),
                    &limits,
                ),
                &mut provider,
            );
            let code = match outcome_error(&result) {
                Some(errors) => Some(TestCheck::item(errors.diagnostics(), 0)?.code()),
                None => None,
            };
            drop(result);
            let state = scope.finish();
            if fail_at.is_none() {
                attempts = Some(state.attempts);
            } else {
                test_eq!(code, Some(DiagnosticCode::ALLOCATION_FAILED))?;
            }
            test_eq!(state.live_bytes, 0)?;
        }
    }

    Ok(())
}

#[test]
fn nested_parser_frames_fail_atomically_on_single_and_persistent_oom() -> TestResult<()> {
    let mut text = String::from("struct Leaf { n: string }\n");
    let mut ty = String::from("Leaf");
    let mut prefix = String::new();
    let mut suffix = String::new();
    for index in 0..24 {
        let name = format!("N{index}");
        text.push_str(&format!(
            "enum {name} {{ V {{ child: option<[{ty}]> }} }}\n"
        ));
        ty = name;
        prefix.push_str(".V{child:[");
        suffix.push_str("]}");
    }
    text.push_str(&format!(
        "entry struct Root {{ ready: [string], deep: {ty}, tail: string }}"
    ));
    let schema = compile(&text)?;
    let source =
        format!("{{ready:[\"already-built\"],deep:{prefix}{{n:\"leaf\"}}{suffix},tail:\"last\"}}");
    let mut limits = ResourceLimits::default();
    limits.set_excerpts(ExcerptLimits::new(0, 0, 0, 0));
    let mut provider = Provider {
        calls: 0,
        result: Ok(None),
    };
    let scope = AllocationScope::start(None);
    drop(
        Support::parse(
            &schema,
            config_request(
                (
                    &mut SliceSource::new(source.as_bytes()),
                    "parser-frames.tconf",
                ),
                &limits,
            ),
            &mut provider,
        )
        .test_value()?,
    );
    let baseline = scope.finish();
    test_eq!(baseline.live_bytes, 0)?;
    for persistent in [false, true] {
        for fail_at in 0..baseline.attempts {
            let scope = AllocationScope::start(Some(fail_at));
            STATE.with(|state| {
                let mut current = state.get();
                current.persistent = persistent;
                state.set(current);
            });
            let mut provider = Provider {
                calls: 0,
                result: Ok(None),
            };
            let result = Support::parse(
                &schema,
                config_request(
                    (
                        &mut SliceSource::new(source.as_bytes()),
                        "parser-frames.tconf",
                    ),
                    &limits,
                ),
                &mut provider,
            );
            let code = outcome_error(&result)
                .and_then(|errors| errors.diagnostics().last())
                .map(|error| error.code());
            drop(result);
            let state = scope.finish();
            test_eq!(code, Some(DiagnosticCode::ALLOCATION_FAILED))?;
            test_eq!(state.live_bytes, 0)?;
        }
    }

    Ok(())
}

#[test]
fn diagnostic_growth_oom_and_inline_terminal_need_no_further_allocation() -> TestResult<()> {
    let schema = compile("entry struct Root { a: u8, b: u8, c: u8 }")?;
    let mut limits = ConfigLimits::default();
    limits = limits.with_max_diagnostics(2);
    let scope = AllocationScope::start(Some(0));
    let errors = parse(&schema, "{ a: \"x\", b: \"y\", c: \"z\" }", &limits).test_error()?;
    test_assert!(!errors.diagnostics().is_empty())?;
    test_eq!(
        errors.diagnostics().last().test_value()?.code(),
        DiagnosticCode::ALLOCATION_FAILED
    )?;
    drop(errors);
    let state = scope.finish();
    test_eq!(state.attempts, 1)?;
    test_eq!(state.live_bytes, 0)?;

    Ok(())
}

fn config_request<'a>(
    source: (&'a mut impl ByteSource, &'a str),
    limits: impl Into<TestLimits>,
) -> ConfigParseRequest<'a> {
    let limits = limits.into();
    ConfigParseRequest::new(
        source.0,
        source.1,
        ConfigParseOptions::new(
            limits.resources(),
            limits.config(),
            ExecutionLimits::new(1_000_000_000, 268_435_456),
        ),
    )
}

fn session_options(limits: impl Into<TestLimits>) -> ConfigParseOptions {
    let limits = limits.into();
    ConfigParseOptions::new(
        limits.resources(),
        limits.config(),
        ExecutionLimits::new(1_000_000_000, 268_435_456),
    )
}

fn dynamic_input(limit: u128, slots: &mut ValueSlots) -> TestResult<ConfigValue> {
    let child_field = slots.field("n", ValueFactory::unsigned(1)?)?;
    let child = slots.structure(vec![child_field])?;
    let child_array = ValueFactory::try_array(vec![child]).map_err(TestFailure::from_debug)?;
    let private_entry = slots.map_entry("private-key", child_array)?;
    let data = slots.map(vec![private_entry])?;
    let limit_field = slots.field("limit", ValueFactory::unsigned(limit)?)?;
    let data_field = slots.field("data", data)?;
    let text = slots.string("aaa")?;
    let text_field = slots.field("text", text)?;
    slots.structure(vec![limit_field, data_field, text_field])
}

#[test]
fn known_resource_failures_precede_allocator_and_provider_side_effects() -> TestResult<()> {
    let schema = compile("entry struct Root {}")?;
    let cases = [
        ConfigParseOptions::new(
            ResourceLimits::default(),
            ConfigLimits::default()
                .with_max_environment_entries(1)
                .with_max_environment_bytes(100)
                .with_max_constructed_text_bytes(100)
                .with_max_display_name_bytes(0),
            ExecutionLimits::new(1_000, 1_048_576),
        ),
        ConfigParseOptions::new(
            ResourceLimits::default(),
            ConfigLimits::default()
                .with_max_environment_entries(1)
                .with_max_environment_bytes(100)
                .with_max_constructed_text_bytes(100)
                .with_max_display_name_bytes(100),
            ExecutionLimits::new(1_000, 0),
        ),
    ];
    for options in cases {
        let mut provider = Provider {
            calls: 0,
            result: Ok(None),
        };
        let scope = AllocationScope::start(Some(0));
        let errors = Support::parse(
            &schema,
            ConfigParseRequest::new(&mut SliceSource::new(("{}").as_bytes()), "test", options),
            &mut provider,
        )
        .test_error()?;
        test_eq!(
            TestCheck::item(errors.diagnostics(), 0)?.code(),
            DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
        )?;
        drop(errors);
        let state = scope.finish();
        test_eq!(state.attempts, 0)?;
        test_eq!(state.live_bytes, 0)?;
        test_eq!(provider.calls, 0)?;
    }

    Ok(())
}

#[test]
fn dynamic_full_tree_normalization_validation_and_snapshots_fail_atomically() -> TestResult<()> {
    let schema = compile(
        r#"struct Child { n: u8 } @assert(self.n > root.limit, "child")
        entry struct Root { limit: u8, data: map<string, [Child]> @secret,
        text: string @regex("^a+$"), defaults: Child = {n: 1} }"#,
    )?;
    let mut slots = ValueSlots::new();
    for limit in [0, 2, 256] {
        let scope = AllocationScope::start(None);
        let input = dynamic_input(limit, &mut slots)?;
        let options = ConfigValidationOptions::default();
        scope.focus(None, false);
        let baseline_result = slots.validate(&schema, input, options);
        let baseline_diagnostic_count =
            outcome_error(&baseline_result).map(|errors| errors.diagnostics().len());
        let baseline_diagnostic_code = outcome_error(&baseline_result)
            .and_then(|errors| errors.diagnostics().first().map(Diagnostic::code));
        let baseline_excerpt_status =
            outcome_error(&baseline_result).map(|errors| errors.excerpts().status());
        drop(baseline_result);
        let baseline = scope.finish();
        test_eq!(baseline.live_bytes, 0)?;
        for persistent in [false, true] {
            for fail_at in 0..baseline.attempts {
                let scope = AllocationScope::start(None);
                let input = dynamic_input(limit, &mut slots)?;
                scope.focus(Some(fail_at), persistent);
                let errors = slots.validate(&schema, input, options).test_error()?;
                let code = errors.diagnostics().last().test_value()?.code();
                if code != DiagnosticCode::ALLOCATION_FAILED {
                    test_assert!(baseline_diagnostic_code.is_some())?;
                    test_eq!(Some(errors.diagnostics().len()), baseline_diagnostic_count)?;
                    test_assert!(
                        errors
                            .diagnostics()
                            .iter()
                            .all(|error| { Some(error.code()) == baseline_diagnostic_code })
                    )?;
                    if baseline_excerpt_status == Some(ExcerptStatus::Complete) {
                        test_assert!(errors.excerpts().status() != ExcerptStatus::Complete)?;
                    }
                } else {
                    test_assert!(
                        errors
                            .diagnostics()
                            .iter()
                            .all(|error| error.path().is_none())
                    )?;
                }
                drop(errors);
                let state = scope.finish();
                test_eq!(state.live_bytes, 0)?;
            }
        }
    }

    Ok(())
}

#[test]
fn snapshot_oom_preserves_original_diagnostic_and_releases_optional_storage() -> TestResult {
    let text = "{value: \"PRIVATE-SENTINEL\"}\n";
    let mut allocation_errors = TestSlot::<AllocationError>::default();
    let mut location_errors = TestSlot::<LocationError>::default();
    let mut span_errors = TestSlot::<SpanError>::default();
    let mut budget_errors = TestSlot::<BudgetError>::default();
    let index = allocation_errors
        .access(|out| LineIndex::new(text, out))
        .map_err(|caught| TestFailure::new(format!("failed to create line index: {:?}", caught)))?;
    let start = text.find('"').test_value()?;
    let end = text.rfind('"').test_value()? + 1;
    let position = span_errors
        .access(|out| Span::new(start, end, out))
        .map_err(|caught| {
            TestFailure::new(format!("failed to create source span: {:?}", caught))
        })?;
    let span = SourceSpan::new(SourceId::new(1), position);
    let location = SpanLocation {
        start: location_errors
            .access(|out| index.location(start, out))
            .map_err(|caught| {
                TestFailure::new(format!(
                    "failed to create source start position: {:?}",
                    caught
                ))
            })?,
        end: location_errors
            .access(|out| index.location(end, out))
            .map_err(|caught| {
                TestFailure::new(format!(
                    "failed to create source end position: {:?}",
                    caught
                ))
            })?,
    };
    let mut diagnostic = budget_errors
        .access(|out| {
            Diagnostic::try_new(
                DiagnosticCode::CONFIG_VALIDATION_ERROR,
                "existing diagnostic",
                None,
                out,
            )
        })
        .map_err(|caught| TestFailure::new(format!("failed to create diagnostic: {:?}", caught)))?;
    diagnostic.set_located_primary(Some(LocatedSpan::new(span, Some(location))));
    let run = || {
        let limits = ExcerptLimits::default();
        let mut capture = SourceCapture::new(span.source_id(), limits, None);
        let Some(prefix) = text.as_bytes().get(..start) else {
            capture.invalidate();
            return false;
        };
        capture.feed(prefix);
        if let Some(id) = capture.begin_secret(start) {
            let Some(secret) = text.as_bytes().get(start..end) else {
                capture.invalidate();
                return false;
            };
            capture.feed(secret);
            capture.end_secret(id, end);
        }
        let Some(tail) = text.as_bytes().get(end..) else {
            capture.invalidate();
            return false;
        };
        capture.feed(tail);
        capture.end_input();
        let mut excerpts = SourceExcerptSet::new(limits, None);
        capture.finish(&mut excerpts, from_ref(&diagnostic));
        let safe = excerpts
            .records()
            .iter()
            .all(|line| !line.text().contains("PRIVATE-SENTINEL"));
        drop(excerpts);
        safe
    };
    let scope = AllocationScope::start(None);
    let safe = run();
    let baseline = scope.finish();
    test_assert!(safe)?;
    test_eq!(baseline.live_bytes, 0)?;
    test_assert!(baseline.attempts > 0)?;
    for persistent in [false, true] {
        for fail_at in 0..baseline.attempts {
            let scope = AllocationScope::start(Some(fail_at));
            STATE.with(|state| {
                let mut current = state.get();
                current.persistent = persistent;
                state.set(current);
            });
            let safe = run();
            let state = scope.finish();
            test_assert!(safe)?;
            test_eq!(state.live_bytes, 0)?;
            test_eq!(diagnostic.code(), DiagnosticCode::CONFIG_VALIDATION_ERROR)?;
            test_eq!(diagnostic.message(), "existing diagnostic")?;
        }
    }
    Ok(())
}
