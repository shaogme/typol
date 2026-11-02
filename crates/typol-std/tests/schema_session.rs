#[macro_use]
#[path = "support/assertions.rs"]
mod assertions;

use opeo::{ErrSlot, OResult, Out};
use std::{
    cell::Cell,
    env::{current_exe, temp_dir, var_os},
    error::Error,
    fmt::Debug,
    fs::{create_dir, remove_dir_all, write},
    io,
    path::{Path, PathBuf},
    process::{Command, id},
    ptr,
    rc::Rc,
    sync::atomic::{AtomicUsize, Ordering},
};

use typol_core::{BudgetError, Diagnostic, DiagnosticCode, ExecutionLimits, SourceId};
use typol_schema::{
    ResolverError, SchemaCompileContext, SchemaCompileErrors, SchemaCompileOptions,
    SchemaCompileStage, SchemaCompiler, SchemaLimits, SchemaResolveRequest, SchemaResolver,
    SchemaSource, SourceLoadedRequest, SourceObserver, SourceObserverError,
};
use typol_semantic::SchemaAccessError;
use typol_std::{
    FileSchemaCompileErrors, FileSchemaCompileFailure, FileSchemaCompileOptions,
    FileSchemaCompiler, FileSchemaLimits, FileSchemaResolver, HostError, HostErrorKind,
    SourceNameEditOptions, SourceNameLimits, SourceNames,
};

const LIMIT: usize = 67_108_864;

trait IntoTestError<T> {
    fn into_test_error(self) -> Result<T, io::Error>;
}

impl<T, E: Debug> IntoTestError<T> for Result<T, E> {
    fn into_test_error(self) -> Result<T, io::Error> {
        self.map_err(|error| io::Error::other(format!("test operation failed: {error:?}")))
    }
}

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Directory {
    path: PathBuf,
}

impl Directory {
    fn new() -> Result<Self, Box<dyn Error>> {
        let path = temp_dir().join(format!(
            "typol-b1-{}-{}",
            id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        create_dir(&path)?;
        Ok(Self { path })
    }

    fn put(&self, name: &str, source: &str) -> Result<PathBuf, Box<dyn Error>> {
        let path = self.path.join(name);
        write(&path, source)?;
        Ok(path)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = remove_dir_all(&self.path);
    }
}

struct NoImports {
    calls: Rc<Cell<usize>>,
}

impl SchemaResolver for NoImports {
    fn resolve<'src, 'slot, 'borrow>(
        &'src mut self,
        _: SchemaResolveRequest<'_>,
        out: Out<'slot, 'borrow, ResolverError>,
    ) -> OResult<'slot, SchemaSource<'src>> {
        self.calls.set(self.calls.get() + 1);
        Err(out.fail(ResolverError::HostFailure))
    }
}

fn new_compiler(steps: usize, bytes: usize) -> Result<SchemaCompiler, Box<dyn Error>> {
    ErrSlot::<SchemaCompileErrors>::new()
        .call(|out| {
            SchemaCompiler::new(
                SchemaCompileOptions::default().with_execution(ExecutionLimits::new(steps, bytes)),
                out,
            )
        })
        .map_err(|error| {
            io::Error::other(format!("Schema initialization failed: {error:?}")).into()
        })
}

fn first_code<'a>(
    diagnostics: impl IntoIterator<Item = &'a Diagnostic>,
) -> Result<DiagnosticCode, io::Error> {
    diagnostics
        .into_iter()
        .next()
        .map(|diagnostic| diagnostic.code())
        .ok_or_else(|| io::Error::other("diagnostic is missing"))
}

#[test]
fn context_clones_keep_identity_and_counters_without_allocating() -> Result<(), Box<dyn Error>> {
    let compiler = new_compiler(LIMIT, LIMIT)?;
    let context = ErrSlot::<BudgetError>::new().call(|out| compiler.context(out))?;
    let clone = ErrSlot::<BudgetError>::new().call(|out| context.try_clone(out))?;
    let initial = context.usage();
    ErrSlot::<BudgetError>::new().call(|out| clone.execution().consume(13, out))?;
    ensure!(context.same_session(&clone));
    ensure_eq!(context.usage().steps, 13);
    ensure_eq!(context.usage().workspace_bytes, initial.workspace_bytes);
    let other = new_compiler(LIMIT, LIMIT)?;
    let other_context = ErrSlot::<BudgetError>::new().call(|out| other.context(out))?;
    ensure!(!context.same_session(&other_context));

    Ok(())
}

#[test]
fn initialization_and_root_checks_precede_any_filesystem_access() -> Result<(), Box<dyn Error>> {
    let missing = Path::new("/path-does-not-exist/secret-sentinel.tschem");
    for bytes in [0, 1] {
        let error = ErrSlot::<SchemaCompileErrors>::new()
            .call(|out| {
                SchemaCompiler::new(
                    SchemaCompileOptions::default()
                        .with_execution(ExecutionLimits::new(LIMIT, bytes)),
                    out,
                )
            })
            .err()
            .ok_or_else(|| io::Error::other("expected Schema initialization to fail"))?;
        ensure_eq!(
            first_code(error.diagnostics())?,
            DiagnosticCode::RESOURCE_LIMIT_EXCEEDED
        );
        let error = ErrSlot::<FileSchemaCompileErrors>::new()
            .call(|out| {
                FileSchemaCompiler::compile(
                    missing,
                    FileSchemaCompileOptions::new(
                        SchemaCompileOptions::default()
                            .with_execution(ExecutionLimits::new(LIMIT, bytes)),
                        FileSchemaLimits::default(),
                        SourceNameLimits::default(),
                    ),
                    out,
                )
            })
            .err()
            .ok_or_else(|| io::Error::other("expected operation to fail"))?;
        ensure_eq!(error.usage().last_stage, SchemaCompileStage::Initialization);
        ensure_eq!(error.usage().workspace_bytes, 0);
        ensure_eq!(error.usage().failure, Some(BudgetError::Resource));
        ensure!(
            matches!(error.failure(), FileSchemaCompileFailure::Compilation(error) if error.diagnostics().first().is_some_and(|diagnostic| diagnostic.primary().is_none()))
        );
    }
    let compiler = new_compiler(0, LIMIT)?;
    let context = ErrSlot::<BudgetError>::new().call(|out| compiler.context(out))?;
    let mut resolver = ErrSlot::<BudgetError>::new()
        .call(|out| FileSchemaResolver::new(FileSchemaLimits::default(), &context, out))?;
    ensure_eq!(
        ErrSlot::<HostError>::new()
            .call(|out| resolver.read_root(missing, out))
            .err()
            .ok_or_else(|| io::Error::other("expected operation to fail"))?
            .kind(),
        HostErrorKind::ResourceLimitExceeded
    );
    ensure_eq!(context.usage().steps, 0);
    let error = ErrSlot::<FileSchemaCompileErrors>::new()
        .call(|out| {
            FileSchemaCompiler::compile(
                missing,
                FileSchemaCompileOptions::new(
                    SchemaCompileOptions::default().with_execution(ExecutionLimits::new(0, LIMIT)),
                    FileSchemaLimits::default(),
                    SourceNameLimits::default(),
                ),
                out,
            )
        })
        .err()
        .ok_or_else(|| io::Error::other("expected operation to fail"))?;
    ensure!(
        matches!(error.failure(), FileSchemaCompileFailure::Root(host) if host.kind() == HostErrorKind::ResourceLimitExceeded)
    );
    ensure_eq!(error.usage().last_stage, SchemaCompileStage::RootRead);
    ensure!(!format!("{error:?}").contains("secret sentinel"));

    Ok(())
}

#[test]
fn terminal_slot_initialization_reports_the_live_usage_snapshot() -> Result<(), Box<dyn Error>> {
    let compiler = new_compiler(LIMIT, LIMIT)?;
    let context = ErrSlot::<BudgetError>::new().call(|out| compiler.context(out))?;
    let initial_workspace = context.usage().workspace_bytes;
    drop(context);
    drop(compiler);

    let error = ErrSlot::<FileSchemaCompileErrors>::new()
        .call(|out| {
            FileSchemaCompiler::compile(
                Path::new("/path-does-not-exist/secret-sentinel.tschem"),
                FileSchemaCompileOptions::new(
                    SchemaCompileOptions::default()
                        .with_execution(ExecutionLimits::new(LIMIT, initial_workspace)),
                    FileSchemaLimits::default(),
                    SourceNameLimits::default(),
                ),
                out,
            )
        })
        .err()
        .ok_or_else(|| {
            io::Error::other(
                "expected terminal slot initialization to fail due to the workspace budget",
            )
        })?;
    let usage = error.usage();
    ensure_eq!(usage.last_stage, SchemaCompileStage::Initialization);
    ensure_eq!(usage.steps, 0);
    ensure_eq!(usage.workspace_bytes, initial_workspace);
    ensure_eq!(usage.peak_workspace_bytes, initial_workspace);
    ensure_eq!(usage.failure, Some(BudgetError::Resource));
    ensure!(
        matches!(error.failure(), FileSchemaCompileFailure::Compilation(errors) if first_code(errors.diagnostics()).ok() == Some(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED))
    );

    Ok(())
}

#[test]
fn root_uses_the_minimum_schema_and_host_source_limit() -> Result<(), Box<dyn Error>> {
    let directory = Directory::new()?;
    let path = directory.put("root.tschem", "entry struct Root {}")?;
    for total in [false, true] {
        let mut resources = SchemaLimits::default();
        if total {
            resources.set_max_schema_total_bytes(1);
        } else {
            resources.set_max_source_bytes(1);
        }
        let error = ErrSlot::<FileSchemaCompileErrors>::new()
            .call(|out| {
                FileSchemaCompiler::compile(
                    &path,
                    FileSchemaCompileOptions::new(
                        SchemaCompileOptions::new(resources),
                        FileSchemaLimits::default(),
                        SourceNameLimits::default(),
                    ),
                    out,
                )
            })
            .err()
            .ok_or_else(|| io::Error::other("expected operation to fail"))?;
        ensure!(
            matches!(error.failure(), FileSchemaCompileFailure::Root(host) if host.kind() == HostErrorKind::ResourceLimitExceeded)
        );
        ensure!(
            error
                .names()
                .ok_or_else(|| io::Error::other("source name record is missing"))?
                .name(SourceId::new(0))
                .is_none()
        );
    }

    Ok(())
}

#[test]
fn mismatched_resolver_observer_and_root_do_no_further_work() -> Result<(), Box<dyn Error>> {
    let first = new_compiler(LIMIT, LIMIT)?;
    let first_context = ErrSlot::<BudgetError>::new().call(|out| first.context(out))?;
    let second = new_compiler(LIMIT, LIMIT)?;
    let second_context = ErrSlot::<BudgetError>::new().call(|out| second.context(out))?;
    let mut resolver = ErrSlot::<BudgetError>::new()
        .call(|out| FileSchemaResolver::new(FileSchemaLimits::default(), &first_context, out))?;
    let mut names = ErrSlot::<BudgetError>::new()
        .call(|out| SourceNames::new(SourceNameLimits::default(), &first_context, out))?;
    let before = first_context.usage();
    ensure!(matches!(
        ErrSlot::<ResolverError>::new().call(|out| resolver.resolve(
            SchemaResolveRequest {
                from_identity: b"invalid",
                module_name: "never",
                source_byte_limit: LIMIT,
                context: &second_context,
            },
            out
        )),
        Err(ResolverError::ContextMismatch)
    ));
    ensure_eq!(
        ErrSlot::<SourceObserverError>::new().call(|out| names.source_loaded(
            SourceLoadedRequest {
                source_id: SourceId::new(0),
                identity: b"root",
                context: &second_context
            },
            out
        )),
        Err(SourceObserverError::HostFailure)
    );
    ensure_eq!(first_context.usage(), before);
    ensure_eq!(second_context.usage().steps, 0);
    let errors = ErrSlot::<SchemaCompileErrors>::new()
        .call(|out| {
            second.compile(
                SchemaSource::in_context(b"root", "extern mod never;", &first_context),
                &mut resolver,
                Some(&mut names),
                out,
            )
        })
        .err()
        .ok_or_else(|| io::Error::other("expected operation to fail"))?;
    ensure_eq!(
        first_code(errors.diagnostics())?,
        DiagnosticCode::SCHEMA_CONTEXT_MISMATCH
    );
    ensure_eq!(second_context.usage().steps, 0);
    ensure_eq!(first_context.usage(), before);
    ensure!(names.name(SourceId::new(0)).is_none());

    Ok(())
}

#[test]
fn mismatched_resolver_is_terminal_before_the_next_import() -> Result<(), Box<dyn Error>> {
    let first = new_compiler(LIMIT, LIMIT)?;
    let context = ErrSlot::<BudgetError>::new().call(|out| first.context(out))?;
    let mut resolver = ErrSlot::<BudgetError>::new()
        .call(|out| FileSchemaResolver::new(FileSchemaLimits::default(), &context, out))?;
    let second = new_compiler(LIMIT, LIMIT)?;
    let errors = ErrSlot::<SchemaCompileErrors>::new()
        .call(|out| {
            second.compile(
                SchemaSource::from_str(
                    b"root",
                    "extern mod first; extern mod later; entry struct Root {}",
                ),
                &mut resolver,
                None,
                out,
            )
        })
        .err()
        .ok_or_else(|| io::Error::other("expected operation to fail"))?;
    ensure_eq!(errors.diagnostics().len(), 1);
    ensure_eq!(
        first_code(errors.diagnostics())?,
        DiagnosticCode::SCHEMA_RESOLVER_ERROR
    );
    ensure_eq!(context.usage().steps, 0);

    Ok(())
}

struct NamesAtLoad<'a> {
    names: &'a mut SourceNames,
    shared: &'a SchemaCompileContext,
    bytes: usize,
    same_context: bool,
}

impl SourceObserver for NamesAtLoad<'_> {
    fn source_loaded<'slot>(
        &mut self,
        request: SourceLoadedRequest<'_>,
        mut out: Out<'slot, '_, SourceObserverError>,
    ) -> OResult<'slot, ()> {
        self.same_context = self.shared.same_session(request.context);
        self.names.source_loaded(request, out.reborrow())?;
        self.bytes = self.shared.usage().workspace_bytes;
        Ok(())
    }
}

struct CombinedMeasurement {
    peak: usize,
    failure: Option<DiagnosticCode>,
    names_growth: bool,
    workspace_released: bool,
    observer_context_matches: bool,
}

fn combined_peak(path: &Path, bytes: usize) -> Result<CombinedMeasurement, Box<dyn Error>> {
    let compiler = new_compiler(LIMIT, bytes)?;
    let context = ErrSlot::<BudgetError>::new().call(|out| compiler.context(out))?;
    let mut resolver = ErrSlot::<BudgetError>::new()
        .call(|out| FileSchemaResolver::new(FileSchemaLimits::default(), &context, out))?;
    let mut root = ErrSlot::<HostError>::new().call(|out| resolver.read_root(path, out))?;
    let retained = context.usage().workspace_bytes;
    let mut names = ErrSlot::<BudgetError>::new()
        .call(|out| SourceNames::new(SourceNameLimits::default(), &context, out))?;
    let mut observer = NamesAtLoad {
        names: &mut names,
        shared: &context,
        bytes: 0,
        same_context: false,
    };
    let result = ErrSlot::<SchemaCompileErrors>::new()
        .call(|out| compiler.compile(root.source(), &mut resolver, Some(&mut observer), out));
    let names_growth = observer.bytes > retained;
    let observer_context_matches = observer.same_context;
    let peak = context.usage().peak_workspace_bytes;
    let code = result.as_ref().err().and_then(|errors| {
        errors
            .diagnostics()
            .last()
            .map(|diagnostic| diagnostic.code())
    });
    drop(result);
    drop(root);
    drop(resolver);
    drop(names);
    let empty_compiler = new_compiler(LIMIT, LIMIT)?;
    let empty_context = ErrSlot::<BudgetError>::new().call(|out| empty_compiler.context(out))?;
    let empty = empty_context.usage().workspace_bytes;
    Ok(CombinedMeasurement {
        peak,
        failure: code,
        names_growth,
        workspace_released: context.usage().workspace_bytes == empty,
        observer_context_matches,
    })
}

#[test]
fn host_streaming_root_ast_and_names_share_the_peak() -> Result<(), Box<dyn Error>> {
    let directory = Directory::new()?;
    let source = format!("{}entry struct Root {{ value: string }}", " ".repeat(8192));
    let path = directory.put("root.tschem", &source)?;
    let file_compiler = new_compiler(LIMIT, LIMIT)?;
    let context = ErrSlot::<BudgetError>::new().call(|out| file_compiler.context(out))?;
    let mut resolver = ErrSlot::<BudgetError>::new()
        .call(|out| FileSchemaResolver::new(FileSchemaLimits::default(), &context, out))?;
    let root = ErrSlot::<HostError>::new().call(|out| resolver.read_root(&path, out))?;
    let file_peak = context.usage().peak_workspace_bytes;
    drop(root);
    drop(resolver);
    let memory = new_compiler(LIMIT, LIMIT)?;
    let context = ErrSlot::<BudgetError>::new().call(|out| memory.context(out))?;
    let import_calls = Rc::new(Cell::new(0));
    let schema = ErrSlot::<SchemaCompileErrors>::new()
        .call(|out| {
            memory.compile(
                SchemaSource::from_str(b"root", &source),
                &mut NoImports {
                    calls: Rc::clone(&import_calls),
                },
                None,
                out,
            )
        })
        .into_test_error()?;
    ensure_eq!(import_calls.get(), 0);
    let memory_peak = context.usage().peak_workspace_bytes;
    drop(schema);
    let shared = combined_peak(&path, LIMIT)?;
    ensure!(shared.workspace_released);
    ensure!(shared.observer_context_matches);
    ensure!(shared.failure.is_none());
    ensure!(shared.names_growth);
    ensure!(shared.peak > file_peak.max(memory_peak));
    ensure_eq!(
        combined_peak(&path, file_peak.max(memory_peak))?.failure,
        Some(DiagnosticCode::RESOURCE_LIMIT_EXCEEDED)
    );

    Ok(())
}

#[test]
fn completed_names_use_new_edit_budgets_and_preserve_the_old_charge() -> Result<(), Box<dyn Error>>
{
    let compiler = new_compiler(LIMIT, LIMIT)?;
    let context = ErrSlot::<BudgetError>::new().call(|out| compiler.context(out))?;
    let initial = context.usage().workspace_bytes;
    let mut names = ErrSlot::<BudgetError>::new()
        .call(|out| SourceNames::new(SourceNameLimits::default(), &context, out))?;
    let import_calls = Rc::new(Cell::new(0));
    let schema = ErrSlot::<SchemaCompileErrors>::new().call(|out| {
        compiler.compile(
            SchemaSource::from_str(b"root", "entry struct Root {}"),
            &mut NoImports {
                calls: Rc::clone(&import_calls),
            },
            Some(&mut names),
            out,
        )
    })?;
    ensure_eq!(import_calls.get(), 0);
    let completed = context.usage();
    let copy = ErrSlot::<HostError>::new()
        .call(|out| names.try_copy(SourceNameEditOptions::default(), out))?;
    ensure_eq!(context.usage(), completed);
    ErrSlot::<HostError>::new().call(|out| {
        names.set_name(
            SourceId::new(0),
            "public alias",
            SourceNameEditOptions::default(),
            out,
        )
    })?;
    ensure_eq!(context.usage().steps, completed.steps);
    ensure!(context.usage().workspace_bytes < completed.workspace_bytes);
    ensure_eq!(copy.name(SourceId::new(0)), Some("schema#0"));
    let before = context.usage();
    ensure_eq!(
        ErrSlot::<SourceObserverError>::new().call(|out| names.source_loaded(
            SourceLoadedRequest {
                source_id: SourceId::new(1),
                identity: b"late",
                context: &context
            },
            out
        )),
        Err(SourceObserverError::HostFailure)
    );
    ensure_eq!(context.usage(), before);
    drop(copy);
    drop(schema);
    drop(names);
    ensure_eq!(context.usage().workspace_bytes, initial);

    Ok(())
}

#[test]
fn high_level_snapshots_follow_temporary_cleanup_on_success_and_failure()
-> Result<(), Box<dyn Error>> {
    let directory = Directory::new()?;
    let path = directory.put("root.tschem", "entry struct Root {}")?;
    let compiled = ErrSlot::<FileSchemaCompileErrors>::new()
        .call(|out| FileSchemaCompiler::compile(&path, FileSchemaCompileOptions::default(), out))?;
    ensure_eq!(
        ErrSlot::<SchemaAccessError>::new().call(|out| compiled.schema().entry().name(out))?,
        Some("Root")
    );
    ensure_eq!(compiled.names().name(SourceId::new(0)), Some("schema#0"));
    ensure_eq!(compiled.usage().last_stage, SchemaCompileStage::Finished);
    ensure!(compiled.usage().workspace_bytes < compiled.usage().peak_workspace_bytes);
    directory.put("root.tschem", "extern mod missing; entry struct Root {}")?;
    let errors = ErrSlot::<FileSchemaCompileErrors>::new()
        .call(|out| FileSchemaCompiler::compile(&path, FileSchemaCompileOptions::default(), out))
        .err()
        .ok_or_else(|| io::Error::other("expected operation to fail"))?;
    ensure!(
        matches!(errors.failure(), FileSchemaCompileFailure::Compilation(error) if error.diagnostics().first().is_some_and(|diagnostic| diagnostic.code() == DiagnosticCode::SCHEMA_SEMANTIC_ERROR))
    );
    let wrapped_diagnostics = errors
        .diagnostics()
        .ok_or_else(|| io::Error::other("wrapper did not expose compilation diagnostics"))?;
    let underlying_diagnostics = match errors.failure() {
        FileSchemaCompileFailure::Compilation(error) => error.diagnostics(),
        FileSchemaCompileFailure::Root(_) => {
            return Err(
                io::Error::other("compilation failure was downgraded to a host error").into(),
            );
        }
    };
    ensure!(ptr::eq(
        wrapped_diagnostics
            .first()
            .ok_or_else(|| io::Error::other("wrapped diagnostics are empty"))?,
        underlying_diagnostics
            .first()
            .ok_or_else(|| io::Error::other("original diagnostics are empty"))?
    ));
    ensure!(errors.usage().workspace_bytes < errors.usage().peak_workspace_bytes);
    let errors = ErrSlot::<FileSchemaCompileErrors>::new()
        .call(|out| {
            FileSchemaCompiler::compile(
                &directory.path.join("missing.tschem"),
                FileSchemaCompileOptions::default(),
                out,
            )
        })
        .err()
        .ok_or_else(|| io::Error::other("expected operation to fail"))?;
    ensure!(
        matches!(errors.failure(), FileSchemaCompileFailure::Root(host) if host.kind() == HostErrorKind::NotFound)
    );
    ensure!(
        errors
            .names()
            .ok_or_else(|| io::Error::other("source name record is missing"))?
            .name(SourceId::new(0))
            .is_none()
    );

    Ok(())
}

#[test]
fn file_session_resource_checks_run_in_a_separate_process() -> Result<(), Box<dyn Error>> {
    if var_os("TYPOL_B1_SESSION_CHILD").is_some() {
        return Ok(());
    }
    let output = Command::new(current_exe()?)
        .args([
            "--test-threads=1",
            "--skip",
            "file_session_resource_checks_run_in_a_separate_process",
        ])
        .env("TYPOL_B1_SESSION_CHILD", "1")
        .output()?;
    ensure!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    Ok(())
}
