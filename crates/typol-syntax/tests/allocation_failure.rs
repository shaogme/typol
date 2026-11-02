use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
    env,
    process::Command,
    ptr::null_mut,
    thread_local,
};
use typol_core::{
    BudgetError, Diagnostic, DiagnosticCode, ExecutionBudget, ExecutionLimits, SliceSource,
    SourceId,
};
use typol_syntax::{
    AnnotationData, AstAccessError, DefaultValueKind, InputConfig, LexEvent, ScalarLiteral,
    SchemaAst, SchemaParseOptions, SchemaParser, StreamLimits, SyntaxErrors, SyntaxLimits,
    TokenCursor,
    opeo::{OResult, Out},
};
use typol_test_support::TestSlot;

struct ParserErrors {
    budget: Option<TestSlot<BudgetError>>,
    syntax: Option<TestSlot<SyntaxErrors>>,
}

impl ParserErrors {
    fn new() -> Self {
        Self {
            budget: None,
            syntax: None,
        }
    }

    fn execution(&mut self, limits: ExecutionLimits) -> Result<ExecutionBudget, BudgetError> {
        let slot = self.budget.get_or_insert_with(TestSlot::new);
        Self::run(slot, |out| ExecutionBudget::new(limits, out))
    }

    fn parse(
        &mut self,
        source_id: SourceId,
        source: &str,
        limits: &SyntaxLimits,
        budget: &ExecutionBudget,
    ) -> Result<SchemaAst, SyntaxErrors> {
        let slot = self.syntax.get_or_insert_with(TestSlot::new);
        let options = SchemaParseOptions::with_syntax(*limits);
        Self::run(slot, |out| {
            SchemaParser::parse_str(source_id, source, &options, budget, out)
        })
    }

    fn run<T, E>(
        slot: &mut TestSlot<E>,
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, E>) -> OResult<'slot, T>,
    ) -> Result<T, E> {
        match slot.access(operation) {
            Ok(value) => Ok(value),
            Err(error) => Err(error),
        }
    }
}

#[derive(Clone, Copy)]
struct AllocationState {
    active: bool,
    fail_at: Option<usize>,
    persistent: bool,
    attempts: usize,
    live_bytes: isize,
    large_allocations: usize,
    peak_bytes: isize,
}

thread_local! {
    static STATE: Cell<AllocationState> = const { Cell::new(AllocationState {
        active: false, fail_at: None, persistent: false, attempts: 0, live_bytes: 0, large_allocations: 0, peak_bytes: 0,
    }) };
}

struct CheckedAllocator;

#[derive(Debug)]
struct TestFailure;

macro_rules! test_assert {
    ($condition:expr $(, $($message:tt)*)?) => {{
        if !$condition {
            return Err(TestFailure);
        }
    }};
}

macro_rules! test_assert_eq {
    ($left:expr, $right:expr $(, $($message:tt)*)?) => {{
        let left = &$left;
        let right = &$right;
        if left != right {
            return Err(TestFailure);
        }
    }};
}

type TestResult<T = ()> = Result<T, TestFailure>;

fn parse(
    errors: &mut ParserErrors,
    source: &str,
    limits: &SyntaxLimits,
    execution: &ExecutionBudget,
) -> Result<SchemaAst, SyntaxErrors> {
    errors.parse(SourceId::new(7), source, limits, execution)
}

fn warm_parser_errors(errors: &mut ParserErrors, execution: &ExecutionBudget) -> TestResult {
    drop(parse(errors, "", &SyntaxLimits::default(), execution));
    Ok(())
}

trait TestValue<T> {
    fn test_value(self) -> TestResult<T>;
}

impl<T, E> TestValue<T> for Result<T, E> {
    fn test_value(self) -> TestResult<T> {
        self.map_err(|_| TestFailure)
    }
}

impl<T> TestValue<T> for Option<T> {
    fn test_value(self) -> TestResult<T> {
        self.ok_or(TestFailure)
    }
}

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
            current.peak_bytes = current.peak_bytes.max(current.live_bytes);
            state.set(current);
        }
    });
}

fn account_large(size: usize) {
    let _ = STATE.try_with(|state| {
        let mut current = state.get();
        if current.active && size >= 65536 {
            current.large_allocations += 1;
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
            account_large(layout.size());
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
            account_large(new_size);
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
                large_allocations: 0,
                peak_bytes: 0,
            })
        });
        Self
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
fn schema_allocation_failures_are_nonempty_and_release_all_storage() -> TestResult {
    if env::var_os("TYPOL_SYNTAX_OOM_CHILD").is_none() {
        return Ok(());
    }
    let mut errors = ParserErrors::new();
    let execution = errors
        .execution(ExecutionLimits::new(67_108_864, 67_108_864))
        .test_value()?;
    warm_parser_errors(&mut errors, &execution)?;

    for source in [
        r#"entry struct Root { name: string = "plain", escaped: string = "a\n€", values: [u8] = [1, 2] } @assert(self.values.len() > 0, "rule")"#,
        r#"type A = i128 @min(-170141183460469231731687303715884105728) @max(0x2a) @range(-1, 2);
        type B = string @length(0, 2) @regex("a\n") @host @email @cidr(v4_only = false) @env("TEXT");
        type C = [u8] @size(max = 2, min = 0) @unique(by = "field") @all(item > 0, "rule") @sum(max = 2) @secret;
        type D = [u8] @size(0, 2) @unique;"#,
        r#"mod A { mod B { type T = option<map<string, [option<u8>; 3]>>;
        struct C { a: u8 = [.V({key: [1, 2]}), .V{key: [3]}], }
        @assert(!(x.f(|v| v.g(1 + 2) > 0).value == 1) || x.f().g(3) > 0, "rule") } }"#,
        "struct A { x: [u8 } struct B { y: }",
        "mod A { struct B { x: u8, y: string = \"a\\n\" } } ? ?",
    ] {
        let limits = SyntaxLimits::default();
        let scope = AllocationScope::start(None);
        drop(parse(&mut errors, source, &limits, &execution));
        let baseline = scope.finish();
        test_assert_eq!(baseline.live_bytes, 0);
        for persistent in [false, true] {
            for fail_at in 0..baseline.attempts {
                let scope = AllocationScope::start(Some(fail_at));
                STATE.with(|state| {
                    let mut current = state.get();
                    current.persistent = persistent;
                    state.set(current);
                });
                let result = parse(&mut errors, source, &limits, &execution);
                let code = result
                    .as_ref()
                    .err()
                    .and_then(|errors| errors.diagnostics().next_back())
                    .map(|diagnostic| diagnostic.code());
                drop(result);
                let state = scope.finish();
                test_assert_eq!(
                    code,
                    Some(DiagnosticCode::ALLOCATION_FAILED),
                    "allocation {fail_at} failed; persistent={persistent}"
                );
                test_assert_eq!(state.live_bytes, 0, "allocation {fail_at} must not leak");
            }
        }
    }

    Ok(())
}

#[test]
fn initialization_failure_needs_exactly_one_allocation_attempt() -> TestResult {
    let mut errors = ParserErrors::new();
    let execution = errors
        .execution(ExecutionLimits::new(67_108_864, 67_108_864))
        .test_value()?;
    warm_parser_errors(&mut errors, &execution)?;

    for budget in [0, 1, 2, 100] {
        let mut limits = SyntaxLimits::default();
        limits.set_max_diagnostics(budget);
        let scope = AllocationScope::start(Some(0));
        STATE.with(|state| {
            let mut current = state.get();
            current.persistent = true;
            state.set(current);
        });
        let result = parse(&mut errors, "", &limits, &execution);
        let code = result.as_ref().err().and_then(|errors| {
            errors
                .diagnostics()
                .first()
                .map(|diagnostic| diagnostic.code())
        });
        drop(result);
        let state = scope.finish();
        test_assert_eq!(code, Some(DiagnosticCode::ALLOCATION_FAILED));
        test_assert_eq!(state.attempts, 1);
        test_assert_eq!(state.live_bytes, 0);
    }

    Ok(())
}

#[test]
fn consuming_a_cached_token_does_not_allocate_for_the_next_string() -> TestResult {
    let mut parser_errors = ParserErrors::new();
    let execution = parser_errors
        .execution(ExecutionLimits::new(67_108_864, 67_108_864))
        .test_value()?;
    warm_parser_errors(&mut parser_errors, &execution)?;
    let mut cursor_errors = TestSlot::<Diagnostic>::default();

    let limits = SyntaxLimits::default();
    let input_limits = limits.input_limits();
    let mut byte_input_1 = SliceSource::new(("before \"a\\n\"").as_bytes());
    let mut cursor = cursor_errors
        .access(|out| {
            TokenCursor::new(
                InputConfig {
                    source_id: SourceId::new(7),
                    source: &mut byte_input_1,
                    resources: &input_limits,
                    stream: StreamLimits::default(),
                    budget: &execution,
                },
                out,
            )
        })
        .test_value()?;
    test_assert!(matches!(
        cursor_errors.access(|out| cursor.peek(out)).test_value()?,
        LexEvent::Token(_)
    ));
    let scope = AllocationScope::start(Some(0));
    let event = cursor_errors.access(|out| cursor.bump(out)).test_value()?;
    let is_token = matches!(event, LexEvent::Token(_));
    drop(event);
    let state = scope.finish();
    test_assert!(is_token);
    test_assert_eq!(state.attempts, 0);
    test_assert!(matches!(
        cursor_errors.access(|out| cursor.peek(out)).test_value()?,
        LexEvent::Token(_)
    ));

    Ok(())
}

#[test]
fn plain_and_escaped_strings_own_payloads_and_move_into_the_ast() -> TestResult {
    let mut parser_errors = ParserErrors::new();
    let execution = parser_errors
        .execution(ExecutionLimits::new(67_108_864, 67_108_864))
        .test_value()?;
    warm_parser_errors(&mut parser_errors, &execution)?;
    let mut cursor_errors = TestSlot::<Diagnostic>::default();

    let limits = SyntaxLimits::default();
    let input_limits = limits.input_limits();
    let mut byte_input_2 = SliceSource::new(("\"plain\"").as_bytes());
    let mut cursor = cursor_errors
        .access(|out| {
            TokenCursor::new(
                InputConfig {
                    source_id: SourceId::new(7),
                    source: &mut byte_input_2,
                    resources: &input_limits,
                    stream: StreamLimits::default(),
                    budget: &execution,
                },
                out,
            )
        })
        .test_value()?;
    let scope = AllocationScope::start(None);
    let token = cursor_errors
        .access(|out| cursor.peek(out))
        .test_value()?
        .token()
        .test_value()?;
    let owned = token.decoded_string().test_value()?.as_ptr() != b"plain".as_ptr();
    let state = scope.finish();
    test_assert!(owned);
    test_assert!(state.attempts > 0);

    for body in ["a".repeat(65536), "\\n".repeat(65536)] {
        let source = format!("type A = string @regex(\"{body}\");");
        let scope = AllocationScope::start(None);
        let parsed = parse(&mut parser_errors, &source, &limits, &execution);
        let succeeded = parsed.is_ok();
        drop(parsed);
        let state = scope.finish();
        test_assert!(succeeded);
        test_assert_eq!(state.live_bytes, 0);
        test_assert_eq!(
            state.large_allocations,
            1,
            "decoded buffer grows on demand without copying large text again into the AST"
        );
    }

    Ok(())
}

#[test]
fn reading_tree_bound_views_does_not_allocate() -> TestResult {
    let mut parser_errors = ParserErrors::new();
    let execution = parser_errors
        .execution(ExecutionLimits::new(67_108_864, 67_108_864))
        .test_value()?;
    warm_parser_errors(&mut parser_errors, &execution)?;

    let ast = parse(
        &mut parser_errors,
        r#"entry struct Root { text: string @regex("€\n₽") = "plain", values: [u8] = [1, 2] }"#,
        &SyntaxLimits::default(),
        &execution,
    )
    .test_value()?;
    let mut ast_errors = TestSlot::<AstAccessError>::default();
    let scope = AllocationScope::start(Some(0));
    STATE.with(|state| {
        let mut current = state.get();
        current.persistent = true;
        state.set(current);
    });
    let item = ast_errors
        .access(|out| ast.items().first(out))
        .test_value()?
        .test_value()?;
    let root = item.entry_struct().test_value()?;
    let fields = ast_errors.access(|out| root.fields(out)).test_value()?;
    let (text, values) = ast_errors
        .access(|mut out| {
            let (text, values) = {
                let mut iter = fields.iter(out.reborrow());
                (iter.next(), iter.next_back())
            };
            let (Some(text), Some(values)) = (text, values) else {
                return Ok(Err(TestFailure));
            };
            let text = text?;
            let values = values?;
            Ok(Ok((text, values)))
        })
        .test_value()?
        .test_value()?;
    let annotations = ast_errors
        .access(|out| text.annotations(out))
        .test_value()?;
    let annotation = ast_errors
        .access(|out| annotations.first(out))
        .test_value()?
        .test_value()?;
    let AnnotationData::Regex(regex) =
        ast_errors.access(|out| annotation.data(out)).test_value()?
    else {
        return Err(TestFailure);
    };
    let default = ast_errors
        .access(|out| text.default(out))
        .test_value()?
        .test_value()?;
    let DefaultValueKind::Scalar(ScalarLiteral::String(plain)) =
        ast_errors.access(|out| default.kind(out)).test_value()?
    else {
        return Err(TestFailure);
    };
    let default = ast_errors
        .access(|out| values.default(out))
        .test_value()?
        .test_value()?;
    let DefaultValueKind::Array(array) = ast_errors.access(|out| default.kind(out)).test_value()?
    else {
        return Err(TestFailure);
    };
    let name = ast_errors.access(|out| root.name(out)).test_value()?;
    let decoded = ast_errors.access(|out| regex.value(out)).test_value()?;
    let raw = ast_errors.access(|out| plain.value(out)).test_value()?;
    let elements = ast_errors.access(|out| array.elements(out)).test_value()?;
    let count = ast_errors
        .access(|out| Ok(elements.iter(out).count()))
        .test_value()?;
    let state = scope.finish();
    test_assert_eq!(state.attempts, 0);
    test_assert_eq!(name, "Root");
    test_assert_eq!(decoded, "€\n₽");
    test_assert_eq!(raw, "plain");
    test_assert_eq!(count, 2);

    Ok(())
}

#[test]
fn collected_error_return_preallocates_terminal_storage_once() -> TestResult {
    let mut errors = ParserErrors::new();
    let execution = errors
        .execution(ExecutionLimits::new(67_108_864, 67_108_864))
        .test_value()?;
    warm_parser_errors(&mut errors, &execution)?;

    let scope = AllocationScope::start(None);
    let result = parse(&mut errors, "?", &SyntaxLimits::default(), &execution);
    let failed = result.is_err();
    drop(result);
    let state = scope.finish();
    test_assert!(failed);
    test_assert_eq!(
        state.attempts,
        7,
        "input window, location index, and lexical diagnostic storage perform only expected allocations"
    );
    test_assert_eq!(state.live_bytes, 0);

    Ok(())
}

#[test]
fn schema_allocation_failures_run_in_an_isolated_process() -> TestResult {
    let status = Command::new(env::current_exe().test_value()?)
        .args([
            "--exact",
            "schema_allocation_failures_are_nonempty_and_release_all_storage",
        ])
        .env("TYPOL_SYNTAX_OOM_CHILD", "1")
        .status()
        .test_value()?;
    test_assert!(
        status.success(),
        "per-allocation OOM child process must exit successfully"
    );

    Ok(())
}

#[test]
fn all_parser_allocations_match_budget_live_and_peak_payloads() -> TestResult {
    let mut errors = ParserErrors::new();
    let execution = errors
        .execution(ExecutionLimits::new(67_108_864, 67_108_864))
        .test_value()?;
    warm_parser_errors(&mut errors, &execution)?;
    for source in [
        r#"entry struct Root { text: string @regex("a\n€") = "plain", values: [u8] = [1, 2] }"#,
        "mod A { type X = option<map<string, [u8]>>; } extern mod B;",
        "struct A { x: u8, y: ? } ? ?",
        "struct A { x: [u8 }",
    ] {
        let execution = errors
            .execution(ExecutionLimits::new(67_108_864, 67_108_864))
            .test_value()?;
        let initial = execution.workspace_bytes();
        let scope = AllocationScope::start(None);
        let result = parse(&mut errors, source, &SyntaxLimits::default(), &execution);
        let current = STATE.with(Cell::get);
        test_assert_eq!(
            current.live_bytes as usize,
            execution.workspace_bytes() - initial
        );
        test_assert_eq!(
            current.peak_bytes as usize,
            execution.peak_workspace_bytes() - initial
        );
        drop(result);
        let state = scope.finish();
        test_assert_eq!(state.live_bytes, 0);
        test_assert_eq!(execution.workspace_bytes(), initial);
    }

    Ok(())
}
