#[macro_use]
#[path = "support/assertions.rs"]
mod assertions;

use std::{
    cell::Cell,
    env::temp_dir,
    error::Error,
    fmt::Debug,
    fs::{copy, create_dir, create_dir_all, hard_link, remove_dir_all, rename, write},
    io::{self, Read as IoRead},
    path::{Path, PathBuf},
    process::id,
    rc::Rc,
    sync::atomic::{AtomicUsize, Ordering},
};

use typol_config::{
    ConfigParseErrors, ConfigParseOptions, ConfigParseRequest, ConfigParser, Environment,
    EnvironmentError, EnvironmentReadContext,
};
use typol_core::{
    BudgetError, DiagnosticCode, ExecutionBudget, ExecutionLimits, Read as SourceRead, SourceId,
    SourceReadError, SourceReadErrorKind, ValueKind,
};
use typol_schema::{
    ResolverError, SchemaCompileContext, SchemaCompileErrors, SchemaCompileOptions, SchemaCompiler,
    SchemaLimits, SchemaResolveRequest, SchemaResolver, SchemaSource, SourceLoadedRequest,
    SourceObserver, SourceObserverError,
};
use typol_semantic::CompiledSchema;
use typol_std::{
    FileConfigInput, FileConfigInputLimits, FileSchemaLimitParams, FileSchemaLimits,
    FileSchemaResolver, FileSchemaSource, HostError, HostErrorKind, HostOperation, ReadSource,
    SourceNameEditOptions, SourceNameLimits, SourceNames,
    opeo::{ErrSlot, OResult, Out},
};

#[cfg(unix)]
use std::{
    ffi::OsString,
    os::unix::{ffi::OsStringExt, fs::symlink},
};

trait IntoTestError<T> {
    fn into_test_error(self) -> Result<T, io::Error>;
}

impl<T, E: Debug> IntoTestError<T> for Result<T, E> {
    fn into_test_error(self) -> Result<T, io::Error> {
        self.map_err(|error| io::Error::other(format!("test operation failed: {error:?}")))
    }
}

fn new_compiler(options: SchemaCompileOptions) -> Result<SchemaCompiler, io::Error> {
    ErrSlot::<SchemaCompileErrors>::new()
        .call(|out| SchemaCompiler::new(options, out))
        .into_test_error()
}

fn compiler_context(compiler: &SchemaCompiler) -> Result<SchemaCompileContext, io::Error> {
    ErrSlot::<BudgetError>::new()
        .call(|out| compiler.context(out))
        .into_test_error()
}

fn new_source_names_with(
    limits: SourceNameLimits,
    context: &SchemaCompileContext,
) -> Result<SourceNames, io::Error> {
    ErrSlot::<BudgetError>::new()
        .call(|out| SourceNames::new(limits, context, out))
        .into_test_error()
}

fn new_source_names(context: &SchemaCompileContext) -> Result<SourceNames, io::Error> {
    new_source_names_with(SourceNameLimits::default(), context)
}

fn new_file_resolver(
    limits: FileSchemaLimits,
    context: &SchemaCompileContext,
) -> Result<FileSchemaResolver, io::Error> {
    ErrSlot::<BudgetError>::new()
        .call(|out| FileSchemaResolver::new(limits, context, out))
        .into_test_error()
}

fn read_root(
    resolver: &mut FileSchemaResolver,
    path: &Path,
) -> Result<FileSchemaSource, HostError> {
    ErrSlot::<HostError>::new().call(|out| resolver.read_root(path, out))
}

fn resolve<'src>(
    resolver: &'src mut FileSchemaResolver,
    request: SchemaResolveRequest<'_>,
) -> Result<SchemaSource<'src>, ResolverError> {
    ErrSlot::<ResolverError>::new().call(|out| resolver.resolve(request, out))
}

fn compile_schema<'src, R: SchemaResolver + ?Sized>(
    compiler: SchemaCompiler,
    root: SchemaSource<'src>,
    resolver: &mut R,
    observer: Option<&mut dyn SourceObserver>,
) -> Result<CompiledSchema, Box<SchemaCompileErrors>> {
    ErrSlot::<SchemaCompileErrors>::new()
        .call(|out| compiler.compile(root, resolver, observer, out))
        .map_err(Box::new)
}

fn source_loaded(
    names: &mut SourceNames,
    request: SourceLoadedRequest<'_>,
) -> Result<(), SourceObserverError> {
    ErrSlot::<SourceObserverError>::new().call(|out| names.source_loaded(request, out))
}

fn set_name(
    names: &mut SourceNames,
    source_id: SourceId,
    name: &str,
    options: SourceNameEditOptions,
) -> Result<(), HostError> {
    ErrSlot::<HostError>::new().call(|out| names.set_name(source_id, name, options, out))
}

fn copy_names(
    names: &SourceNames,
    options: SourceNameEditOptions,
) -> Result<SourceNames, HostError> {
    ErrSlot::<HostError>::new().call(|out| names.try_copy(options, out))
}

fn open_config_input(
    path: &Path,
    limits: FileConfigInputLimits,
    source_byte_limit: usize,
    budget: &ExecutionBudget,
) -> Result<FileConfigInput, HostError> {
    ErrSlot::<HostError>::new()
        .call(|out| FileConfigInput::open(path, limits, source_byte_limit, budget, out))
}

fn read_source<S: SourceRead<SourceReadError> + ?Sized>(
    source: &mut S,
    output: &mut [u8],
    source_id: SourceId,
    offset: usize,
) -> Result<usize, SourceReadError> {
    ErrSlot::<SourceReadError>::new().call(|out| source.read(output, source_id, offset, out))
}

fn new_read_source<'reader, R: IoRead>(
    reader: &'reader mut R,
    budget: &ExecutionBudget,
) -> Result<ReadSource<'reader>, SourceReadError> {
    ErrSlot::<SourceReadError>::new().call(|out| ReadSource::new(reader, budget, out))
}

fn new_budget(limits: ExecutionLimits) -> Result<ExecutionBudget, io::Error> {
    ErrSlot::<BudgetError>::new()
        .call(|out| ExecutionBudget::new(limits, out))
        .into_test_error()
}

const DEFAULT_TEST_EXECUTION: ExecutionLimits = ExecutionLimits::new(16_777_216, 16_777_216);

static NEXT_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

struct Directory {
    path: PathBuf,
}

impl Directory {
    fn new() -> Result<Self, Box<dyn Error>> {
        let path = temp_dir().join(format!(
            "typol-files-{}-{}",
            id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
        ));
        create_dir(&path)?;
        Ok(Self { path })
    }

    fn put(&self, path: &str, text: impl AsRef<[u8]>) -> Result<PathBuf, Box<dyn Error>> {
        let path = self.path.join(path);
        let parent = path
            .parent()
            .ok_or_else(|| io::Error::other("test path is missing its parent directory"))?;
        create_dir_all(parent)?;
        write(&path, text)?;
        Ok(path)
    }

    fn compile(
        &self,
        path: &str,
        names: &mut SourceNames,
        compiler: SchemaCompiler,
    ) -> Result<Result<CompiledSchema, Box<SchemaCompileErrors>>, Box<dyn Error>> {
        let context = compiler_context(&compiler)
            .map_err(|error| io::Error::other(format!("context creation failed: {error:?}")))?;
        let mut resolver = new_file_resolver(FileSchemaLimits::default(), &context)
            .map_err(|error| io::Error::other(format!("parser creation failed: {error:?}")))?;
        let mut root = read_root(&mut resolver, &self.path.join(path))
            .map_err(|error| io::Error::other(format!("root Schema read failed: {error:?}")))?;
        Ok(compile_schema(
            compiler,
            root.source(),
            &mut resolver,
            Some(names),
        ))
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = remove_dir_all(&self.path);
    }
}

fn defaults() -> FileSchemaLimitParams {
    let limits = FileSchemaLimits::default();
    FileSchemaLimitParams {
        max_file_bytes: limits.max_file_bytes(),
        max_files: limits.max_files(),
        max_path_bytes: limits.max_path_bytes(),
        max_name_bytes: limits.max_name_bytes(),
    }
}

#[test]
fn original_schemas_compile_from_real_directories() -> Result<(), Box<dyn Error>> {
    let directory = Directory::new()?;
    for name in ["server", "database", "all_types", "common"] {
        let filename = format!("{name}.tschem");
        copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../examples/config")
                .join(&filename),
            directory.path.join(filename),
        )?;
    }
    for name in ["server", "database", "all_types"] {
        let compiler = new_compiler(SchemaCompileOptions::default())?;
        let context = compiler_context(&compiler)?;
        let mut names = new_source_names(&context)?;
        directory
            .compile(&format!("{name}.tschem"), &mut names, compiler)
            .into_test_error()?
            .into_test_error()?;
        ensure_eq!(names.failure(), None);
        ensure_eq!(names.name(SourceId::new(0)), Some("schema#0"));
        ensure_eq!(
            names.name(SourceId::new(1)),
            (name != "all_types").then_some("schema#1")
        );
    }

    Ok(())
}

#[test]
fn preferred_candidate_wins_and_missing_preferred_uses_stem_directory() -> Result<(), Box<dyn Error>>
{
    let compiler = new_compiler(SchemaCompileOptions::default())?;
    let context = compiler_context(&compiler)?;

    let directory = Directory::new()?;
    directory.put(
        "root.tschem",
        "extern mod child; entry struct Root { item: child::Child }",
    )?;
    directory.put("child.tschem", "struct Child { value: u8 }")?;
    directory.put("root/child.tschem", "?")?;
    directory
        .compile("root.tschem", &mut new_source_names(&context)?, compiler)
        .into_test_error()?
        .into_test_error()?;
    let compiler = new_compiler(SchemaCompileOptions::default())?;
    let context = compiler_context(&compiler)?;
    let directory = Directory::new()?;
    directory.put(
        "root.tschem",
        "extern mod child; entry struct Root { item: child::Child }",
    )?;
    directory.put("root/child.tschem", "struct Child { value: u8 }")?;
    directory
        .compile("root.tschem", &mut new_source_names(&context)?, compiler)
        .into_test_error()?
        .into_test_error()?;

    Ok(())
}

#[test]
fn directory_invalid_utf8_and_parse_errors_never_fall_back() -> Result<(), Box<dyn Error>> {
    for kind in ["directory", "utf8", "syntax"] {
        let compiler = new_compiler(SchemaCompileOptions::default())?;
        let context = compiler_context(&compiler)?;
        let directory = Directory::new()?;
        directory.put(
            "root.tschem",
            "extern mod child; entry struct Root { item: child::Child }",
        )?;
        directory.put("root/child.tschem", "struct Child { value: u8 }")?;
        match kind {
            "directory" => create_dir(directory.path.join("child.tschem"))?,
            "utf8" => {
                directory.put("child.tschem", [255])?;
            }
            _ => {
                directory.put("child.tschem", "?")?;
            }
        }
        let mut names = new_source_names(&context)?;
        let errors = directory
            .compile("root.tschem", &mut names, compiler)?
            .err()
            .ok_or_else(|| io::Error::other("expected operation to fail"))?;
        ensure!(
            errors
                .diagnostics()
                .iter()
                .any(|diagnostic| diagnostic.code()
                    == if kind != "directory" {
                        DiagnosticCode::LEXICAL_ERROR
                    } else {
                        DiagnosticCode::SCHEMA_RESOLVER_ERROR
                    })
        );
        ensure_eq!(
            names.name(SourceId::new(1)),
            (kind != "directory").then_some("schema#1")
        );
    }

    Ok(())
}

#[test]
fn recursive_imports_use_current_target_stem() -> Result<(), Box<dyn Error>> {
    let compiler = new_compiler(SchemaCompileOptions::default())?;
    let context = compiler_context(&compiler)?;

    let directory = Directory::new()?;
    directory.put(
        "root.tschem",
        "extern mod child; entry struct Root { item: child::Child }",
    )?;
    directory.put(
        "root/child.tschem",
        "extern mod leaf; struct Child { item: leaf::Leaf }",
    )?;
    directory.put("root/child/leaf.tschem", "struct Leaf { value: u8 }")?;
    let mut names = new_source_names(&context)?;
    directory
        .compile("root.tschem", &mut names, compiler)
        .into_test_error()?
        .into_test_error()?;
    ensure_eq!(names.name(SourceId::new(2)), Some("schema#2"));
    ensure_eq!(names.name(SourceId::new(3)), None);

    Ok(())
}

#[test]
fn file_schema_source_is_reopened_without_content_cache() -> Result<(), Box<dyn Error>> {
    let compiler = new_compiler(SchemaCompileOptions::default())?;
    let context = compiler_context(&compiler)?;

    let directory = Directory::new()?;
    let root_path = directory.put("root.tschem", "entry struct Root {}")?;
    directory.put("child.tschem", "struct Child {}")?;
    let mut resolver = new_file_resolver(FileSchemaLimits::default(), &context)?;
    let root = read_root(&mut resolver, &root_path)?;
    {
        let _first = resolve(
            &mut resolver,
            SchemaResolveRequest {
                from_identity: &0_u64.to_le_bytes(),
                module_name: "child",
                source_byte_limit: 100,
                context: &context,
            },
        )
        .into_test_error()?;
    }
    directory.put("child.tschem", [255])?;
    let changed = resolve(
        &mut resolver,
        SchemaResolveRequest {
            from_identity: &0_u64.to_le_bytes(),
            module_name: "child",
            source_byte_limit: 100,
            context: &context,
        },
    )
    .into_test_error()?;
    let errors = ErrSlot::<SchemaCompileErrors>::new()
        .call(|out| compiler.compile_without_imports(changed, out))
        .err()
        .ok_or_else(|| {
            io::Error::other("re-reading invalid UTF-8 should produce a lexical diagnostic")
        })?;
    ensure!(
        errors
            .diagnostics()
            .iter()
            .any(|diagnostic| diagnostic.code() == DiagnosticCode::LEXICAL_ERROR)
    );
    ensure!(
        matches!(read_root(&mut resolver, &root_path), Err(error) if error.kind() == HostErrorKind::InvalidRequest)
    );
    drop(root);

    Ok(())
}

#[test]
fn direct_requests_reject_paths_keywords_and_unknown_identities() -> Result<(), Box<dyn Error>> {
    let compiler = new_compiler(SchemaCompileOptions::default())?;
    let context = compiler_context(&compiler)?;

    let directory = Directory::new()?;
    let path = directory.put("root.tschem", "entry struct Root {}")?;
    let mut resolver = new_file_resolver(FileSchemaLimits::default(), &context)?;
    let _root = read_root(&mut resolver, &path)?;
    for name in [
        "", ".", "..", "../child", "/child", "a/b", "a\\b", "é", "entry", "u8", "none",
    ] {
        ensure!(
            matches!(
                resolve(
                    &mut resolver,
                    SchemaResolveRequest {
                        from_identity: &0_u64.to_le_bytes(),
                        module_name: name,
                        source_byte_limit: 100,
                        context: &context
                    }
                ),
                Err(ResolverError::HostFailure)
            ),
            "{name}"
        );
    }
    for identity in [&b""[..], &b"path"[..], &999_u64.to_le_bytes()[..]] {
        ensure!(matches!(
            resolve(
                &mut resolver,
                SchemaResolveRequest {
                    from_identity: identity,
                    module_name: "child",
                    source_byte_limit: 100,
                    context: &context
                }
            ),
            Err(ResolverError::HostFailure)
        ));
    }
    ensure!(matches!(
        resolve(
            &mut resolver,
            SchemaResolveRequest {
                from_identity: &0_u64.to_le_bytes(),
                module_name: "missing",
                source_byte_limit: 100,
                context: &context
            }
        ),
        Err(ResolverError::NotFound)
    ));

    Ok(())
}

#[test]
fn zero_exact_and_excess_file_lengths_are_distinguished() -> Result<(), Box<dyn Error>> {
    let directory = Directory::new()?;
    let path = directory.put("config.tconf", "")?;
    let budget = new_budget(DEFAULT_TEST_EXECUTION)?;
    let mut source = open_config_input(&path, FileConfigInputLimits::default(), 0, &budget)?;
    ensure_eq!(
        read_source(&mut source, &mut [0; 1], SourceId::new(0), 0)?,
        0
    );
    write(&path, "Ḁ")?;
    let mut source = open_config_input(&path, FileConfigInputLimits::default(), 3, &budget)?;
    let mut bytes = [0; 3];
    ensure_eq!(
        read_source(&mut source, &mut bytes, SourceId::new(0), 0)?,
        3
    );
    ensure_eq!(&bytes, "Ḁ".as_bytes());
    ensure_eq!(
        read_source(&mut source, &mut bytes, SourceId::new(0), 3)?,
        0
    );
    let canonical_path = path.canonicalize()?;
    ensure!(source.path() == path.as_path() || source.path() == canonical_path.as_path());
    for limit in [0, 2] {
        let error = open_config_input(&path, FileConfigInputLimits::default(), limit, &budget)
            .err()
            .ok_or_else(|| io::Error::other("expected operation to fail"))?;
        ensure_eq!(error.kind(), HostErrorKind::ResourceLimitExceeded);
        ensure_eq!(error.operation(), HostOperation::ConfigRead);
        ensure!(error.source().is_none());
    }
    write(&path, [255])?;
    let mut source = open_config_input(&path, FileConfigInputLimits::default(), 1, &budget)?;
    ensure_eq!(
        read_source(&mut source, &mut bytes, SourceId::new(0), 0)?,
        1
    );
    ensure_eq!(bytes[0], 255);

    Ok(())
}

#[test]
fn file_config_input_file_and_path_limits_are_enforced() -> Result<(), Box<dyn Error>> {
    let directory = Directory::new()?;
    let path = directory.put("bounded.tconf", b"{}")?;
    let budget = new_budget(DEFAULT_TEST_EXECUTION)?;

    let exact_file_limit = FileConfigInputLimits::default().with_max_file_bytes(2);
    let mut input = open_config_input(&path, exact_file_limit, usize::MAX, &budget)?;
    let mut bytes = [0; 2];
    ensure_eq!(read_source(&mut input, &mut bytes, SourceId::new(0), 0)?, 2);
    ensure_eq!(&bytes, b"{}");

    let short_file_limit = FileConfigInputLimits::default().with_max_file_bytes(1);
    let error = open_config_input(&path, short_file_limit, usize::MAX, &budget)
        .err()
        .ok_or_else(|| io::Error::other("file byte limit should reject a larger file"))?;
    ensure_eq!(error.kind(), HostErrorKind::ResourceLimitExceeded);

    let no_path_limit = FileConfigInputLimits::default().with_max_path_bytes(0);
    let error = open_config_input(&path, no_path_limit, usize::MAX, &budget)
        .err()
        .ok_or_else(|| io::Error::other("path byte limit should reject a non-empty path"))?;
    ensure_eq!(error.kind(), HostErrorKind::ResourceLimitExceeded);

    Ok(())
}

#[test]
fn host_file_path_count_step_and_workspace_limits_are_enforced() -> Result<(), Box<dyn Error>> {
    let compiler = new_compiler(SchemaCompileOptions::default())?;
    let context = compiler_context(&compiler)?;

    let directory = Directory::new()?;
    let path = directory.put("root.tschem", "extern mod child; entry struct Root {}")?;
    directory.put("child.tschem", "struct Child {}")?;
    let mut params = defaults();
    params.max_files = 1;
    let mut resolver = new_file_resolver(
        FileSchemaLimits::new(FileSchemaLimitParams {
            max_file_bytes: params.max_file_bytes,
            max_files: params.max_files,
            max_path_bytes: params.max_path_bytes,
            max_name_bytes: params.max_name_bytes,
        }),
        &context,
    )?;
    let _root = read_root(&mut resolver, &path)?;
    ensure!(matches!(
        resolve(
            &mut resolver,
            SchemaResolveRequest {
                from_identity: &0_u64.to_le_bytes(),
                module_name: "child",
                source_byte_limit: 100,
                context: &context
            }
        ),
        Err(ResolverError::ResourceLimitExceeded)
    ));
    for field in ["files", "path", "bytes"] {
        let mut params = defaults();
        match field {
            "files" => params.max_files = 0,
            "path" => params.max_path_bytes = 0,
            _ => params.max_file_bytes = 1,
        }
        let mut resolver = new_file_resolver(
            FileSchemaLimits::new(FileSchemaLimitParams {
                max_file_bytes: params.max_file_bytes,
                max_files: params.max_files,
                max_path_bytes: params.max_path_bytes,
                max_name_bytes: params.max_name_bytes,
            }),
            &context,
        )?;
        let error = read_root(&mut resolver, &path)
            .err()
            .ok_or_else(|| io::Error::other("expected operation to fail"))?;
        ensure_eq!(
            error.kind(),
            HostErrorKind::ResourceLimitExceeded,
            "{field}"
        );
    }

    Ok(())
}

#[test]
fn source_names_use_actual_ids_and_copy_without_aliasing() -> Result<(), Box<dyn Error>> {
    let compiler = new_compiler(SchemaCompileOptions::default())?;
    let context = compiler_context(&compiler)?;

    let mut names = new_source_names(&context)?;
    for id in (0..64).rev() {
        source_loaded(
            &mut names,
            SourceLoadedRequest {
                source_id: SourceId::new(id),
                identity: b"not-a-file-identity",
                context: &context,
            },
        )
        .into_test_error()?;
    }
    source_loaded(
        &mut names,
        SourceLoadedRequest {
            source_id: SourceId::new(12),
            identity: b"other",
            context: &context,
        },
    )
    .into_test_error()?;
    ensure_eq!(names.name(SourceId::new(64)), None);
    for id in 0..64 {
        let expected = format!("schema#{id}");
        ensure_eq!(names.name(SourceId::new(id)), Some(expected.as_str()));
    }
    ensure_eq!(names.name(SourceId::new(12)), Some("schema#12"));
    drop(compiler);
    set_name(
        &mut names,
        SourceId::new(12),
        "public alias\n",
        SourceNameEditOptions::default(),
    )?;
    let copy = copy_names(&names, SourceNameEditOptions::default())?;
    ensure_eq!(copy.failure(), None);
    set_name(
        &mut names,
        SourceId::new(12),
        "changed",
        SourceNameEditOptions::default(),
    )?;
    ensure_eq!(copy.name(SourceId::new(12)), Some("public alias\n"));
    ensure!(!format!("{copy:?}").contains("public"));
    ensure_eq!(
        set_name(
            &mut names,
            SourceId::new(64),
            "future",
            SourceNameEditOptions::default()
        )
        .err()
        .ok_or_else(|| io::Error::other("expected operation to fail"))?
        .kind(),
        HostErrorKind::InvalidRequest
    );

    Ok(())
}

#[test]
fn observer_latches_first_failure_and_alias_failure_keeps_old_name() -> Result<(), Box<dyn Error>> {
    let compiler = new_compiler(SchemaCompileOptions::default())?;
    let context = compiler_context(&compiler)?;

    let mut params = defaults();
    params.max_name_bytes = 8;
    let mut names = new_source_names_with(
        SourceNameLimits::new(params.max_files, params.max_name_bytes),
        &context,
    )?;
    source_loaded(
        &mut names,
        SourceLoadedRequest {
            source_id: SourceId::new(0),
            identity: b"root",
            context: &context,
        },
    )
    .into_test_error()?;
    ensure_eq!(
        set_name(
            &mut names,
            SourceId::new(0),
            "too-long-name",
            SourceNameEditOptions::default(),
        )
        .err()
        .ok_or_else(|| io::Error::other("expected operation to fail"))?
        .kind(),
        HostErrorKind::InvalidRequest
    );
    ensure_eq!(
        set_name(
            &mut names,
            SourceId::new(0),
            "too-long-name",
            SourceNameEditOptions::new(
                SourceNameLimits::new(64, 8),
                ExecutionLimits::new(1000, 1000)
            ),
        )
        .err()
        .ok_or_else(|| io::Error::other("expected operation to fail"))?
        .kind(),
        HostErrorKind::InvalidRequest
    );
    ensure_eq!(names.failure(), None);
    ensure_eq!(names.name(SourceId::new(0)), Some("schema#0"));
    ensure_eq!(
        source_loaded(
            &mut names,
            SourceLoadedRequest {
                source_id: SourceId::new(10),
                identity: b"child",
                context: &context
            }
        ),
        Err(SourceObserverError::ResourceLimitExceeded)
    );
    ensure_eq!(
        source_loaded(
            &mut names,
            SourceLoadedRequest {
                source_id: SourceId::new(1),
                identity: b"later",
                context: &context
            }
        ),
        Err(SourceObserverError::ResourceLimitExceeded)
    );
    ensure_eq!(names.failure(), Some(HostErrorKind::ResourceLimitExceeded));
    let failure = names
        .failure_context()
        .ok_or_else(|| io::Error::other("source registration failure context is missing"))?;
    ensure_eq!(failure.operation(), HostOperation::SourceNames);
    ensure_eq!(failure.source_id(), Some(SourceId::new(10)));
    ensure_eq!(failure.context().failure_point(), "name-table-operation");
    ensure!(!format!("{failure:?} {failure}").contains("schema#1"));
    ensure_eq!(names.name(SourceId::new(1)), None);
    drop(compiler);
    let completed = context.usage();
    ensure_eq!(
        set_name(
            &mut names,
            SourceId::new(0),
            "x",
            SourceNameEditOptions::default()
        )
        .err()
        .ok_or_else(|| io::Error::other("expected operation to fail"))?
        .kind(),
        HostErrorKind::ResourceLimitExceeded
    );
    ensure_eq!(context.usage(), completed);
    let copy = copy_names(&names, SourceNameEditOptions::default())?;
    let mut copy = copy_names(&copy, SourceNameEditOptions::default())?;
    drop(names);
    ensure_eq!(copy.name(SourceId::new(0)), Some("schema#0"));
    ensure_eq!(copy.name(SourceId::new(10)), None);
    ensure_eq!(copy.failure(), Some(HostErrorKind::ResourceLimitExceeded));
    ensure_eq!(
        set_name(
            &mut copy,
            SourceId::new(0),
            "x",
            SourceNameEditOptions::default()
        )
        .err()
        .ok_or_else(|| io::Error::other("expected operation to fail"))?
        .kind(),
        HostErrorKind::ResourceLimitExceeded
    );
    ensure_eq!(copy.name(SourceId::new(0)), Some("schema#0"));
    ensure_eq!(context.usage().steps, completed.steps);
    ensure_eq!(
        copy_names(
            &copy,
            SourceNameEditOptions::new(
                SourceNameLimits::new(0, 0),
                ExecutionLimits::new(1000, 1000)
            )
        )
        .err()
        .ok_or_else(|| io::Error::other("expected operation to fail"))?
        .kind(),
        HostErrorKind::ResourceLimitExceeded
    );
    ensure_eq!(copy.failure(), Some(HostErrorKind::ResourceLimitExceeded));
    ensure!(
        ErrSlot::<SchemaCompileErrors>::new()
            .call(|out| SchemaCompiler::new(
                SchemaCompileOptions::default().with_execution(ExecutionLimits::new(100, 0)),
                out,
            ))
            .is_err()
    );

    Ok(())
}

#[test]
fn compile_failure_keeps_positions_after_all_input_storage_is_released()
-> Result<(), Box<dyn Error>> {
    let compiler = new_compiler(SchemaCompileOptions::default())?;
    let context = compiler_context(&compiler)?;

    let directory = Directory::new()?;
    directory.put("root.tschem", "extern mod child; entry struct Root {}\r\n")?;
    directory.put(
        "child.tschem",
        "// marker\r\nstruct Broken { value: Unknown }\r\n",
    )?;
    let mut names = new_source_names(&context)?;
    let errors = directory
        .compile("root.tschem", &mut names, compiler)?
        .err()
        .ok_or_else(|| io::Error::other("expected operation to fail"))?;
    drop(directory);
    let span = errors
        .diagnostics()
        .first()
        .ok_or_else(|| io::Error::other("diagnostic is missing"))?
        .primary()
        .ok_or_else(|| io::Error::other("primary location is missing"))?;
    ensure_eq!(span.source_id(), SourceId::new(1));
    ensure!(errors.span_location(span).is_some());
    ensure_eq!(names.name(span.source_id()), Some("schema#1"));

    Ok(())
}

#[test]
fn names_failure_aborts_before_parsing_root_or_reading_imports() -> Result<(), Box<dyn Error>> {
    let compiler = new_compiler(SchemaCompileOptions::default())?;
    let context = compiler_context(&compiler)?;

    let directory = Directory::new()?;
    directory.put("root.tschem", "extern mod child; entry struct Root {}")?;
    directory.put("child.tschem", "?")?;
    let mut params = defaults();
    params.max_files = 0;
    let mut names = new_source_names_with(
        SourceNameLimits::new(params.max_files, params.max_name_bytes),
        &context,
    )?;
    let errors = directory
        .compile("root.tschem", &mut names, compiler)?
        .err()
        .ok_or_else(|| io::Error::other("expected operation to fail"))?;
    ensure_eq!(names.failure(), Some(HostErrorKind::ResourceLimitExceeded));
    ensure_eq!(errors.diagnostics().len(), 1);
    let diagnostic = errors
        .diagnostics()
        .first()
        .ok_or_else(|| io::Error::other("diagnostic is missing"))?;
    ensure_eq!(diagnostic.code(), DiagnosticCode::RESOURCE_LIMIT_EXCEEDED);
    let primary = diagnostic
        .primary()
        .ok_or_else(|| io::Error::other("primary location is missing"))?;
    ensure_eq!(primary.source_id(), SourceId::new(0));
    ensure!(errors.span_location(primary).is_none());
    ensure_eq!(names.name(SourceId::new(1)), None);

    Ok(())
}

struct NoEnvironment {
    calls: Rc<Cell<usize>>,
}

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

#[test]
fn config_file_source_connects_to_public_parser() -> Result<(), Box<dyn Error>> {
    let compiler = new_compiler(SchemaCompileOptions::default())?;
    let context = compiler_context(&compiler)?;

    let directory = Directory::new()?;
    let path = directory.put("root.tschem", "entry struct Root { value: u8 }")?;
    let config_path = directory.put("config.tconf", "{value: 7}")?;
    let mut resolver = new_file_resolver(FileSchemaLimits::default(), &context)?;
    let mut root = read_root(&mut resolver, &path)?;
    let schema = compile_schema(compiler, root.source(), &mut resolver, None).into_test_error()?;
    let mut config = open_config_input(
        &config_path,
        FileConfigInputLimits::default(),
        100,
        &new_budget(DEFAULT_TEST_EXECUTION)?,
    )?;
    let environment_calls = Rc::new(Cell::new(0));
    let mut environment = NoEnvironment {
        calls: Rc::clone(&environment_calls),
    };
    let value = ErrSlot::<ConfigParseErrors>::new()
        .call(|out| {
            ConfigParser::new(&schema).parse(
                ConfigParseRequest::new(&mut config, "config", ConfigParseOptions::default()),
                &mut environment,
                out,
            )
        })
        .into_test_error()?;
    drop(config);
    ensure_eq!(environment_calls.get(), 0);
    let value = value.into_config_value();
    ensure!(matches!(value.as_ref(), ValueKind::Structure(_)));

    Ok(())
}

#[cfg(unix)]
#[test]
fn symlink_root_uses_target_directory_and_symlink_duplicates_share_identity()
-> Result<(), Box<dyn Error>> {
    let compiler = new_compiler(SchemaCompileOptions::default())?;
    let context = compiler_context(&compiler)?;

    let directory = Directory::new()?;
    directory.put(
        "target/root.tschem",
        "extern mod child; entry struct Root { item: child::Child }",
    )?;
    directory.put("target/child.tschem", "struct Child {}")?;
    directory.put("child.tschem", "?")?;
    symlink(
        directory.path.join("target/root.tschem"),
        directory.path.join("link.tschem"),
    )?;
    directory
        .compile("link.tschem", &mut new_source_names(&context)?, compiler)
        .into_test_error()?
        .into_test_error()?;
    directory.put(
        "target/root.tschem",
        "extern mod child; extern mod alias; entry struct Root {}",
    )?;
    symlink(
        directory.path.join("target/child.tschem"),
        directory.path.join("target/alias.tschem"),
    )?;
    let compiler = new_compiler(SchemaCompileOptions::default())?;
    let context = compiler_context(&compiler)?;
    let mut names = new_source_names(&context)?;
    let errors = directory
        .compile("link.tschem", &mut names, compiler)?
        .err()
        .ok_or_else(|| io::Error::other("expected operation to fail"))?;
    ensure!(errors.diagnostics().iter().any(|diagnostic| {
        diagnostic
            .message()
            .contains("the same schema source identity was imported more than once")
    }));
    ensure_eq!(names.name(SourceId::new(2)), None);

    Ok(())
}

#[cfg(unix)]
#[test]
fn import_of_root_symlink_is_a_cycle_and_dangling_symlink_falls_back() -> Result<(), Box<dyn Error>>
{
    let compiler = new_compiler(SchemaCompileOptions::default())?;
    let context = compiler_context(&compiler)?;

    let directory = Directory::new()?;
    directory.put("root.tschem", "extern mod child; entry struct Root {}")?;
    symlink(
        directory.path.join("root.tschem"),
        directory.path.join("child.tschem"),
    )?;
    let mut names = new_source_names(&context)?;
    let errors = directory
        .compile("root.tschem", &mut names, compiler)?
        .err()
        .ok_or_else(|| io::Error::other("expected operation to fail"))?;
    ensure!(errors.diagnostics().iter().any(|diagnostic| {
        diagnostic
            .message()
            .contains("schema import cycle detected")
    }));
    ensure_eq!(names.name(SourceId::new(1)), None);
    let compiler = new_compiler(SchemaCompileOptions::default())?;
    let context = compiler_context(&compiler)?;
    let directory = Directory::new()?;
    directory.put("root.tschem", "extern mod child; entry struct Root {}")?;
    directory.put("root/child.tschem", "struct Child {}")?;
    symlink(
        directory.path.join("missing"),
        directory.path.join("child.tschem"),
    )?;
    directory
        .compile("root.tschem", &mut new_source_names(&context)?, compiler)
        .into_test_error()?
        .into_test_error()?;

    Ok(())
}

#[test]
fn hard_link_imports_share_the_opened_file_identity() -> Result<(), Box<dyn Error>> {
    let compiler = new_compiler(SchemaCompileOptions::default())?;
    let context = compiler_context(&compiler)?;

    let directory = Directory::new()?;
    directory.put(
        "root.tschem",
        "extern mod child; extern mod alias; entry struct Root {}",
    )?;
    directory.put("child.tschem", "struct Child {}")?;
    hard_link(
        directory.path.join("child.tschem"),
        directory.path.join("alias.tschem"),
    )?;
    let mut names = new_source_names(&context)?;
    let errors = directory
        .compile("root.tschem", &mut names, compiler)?
        .err()
        .ok_or_else(|| {
            io::Error::other("importing a hard link twice should produce a diagnostic")
        })?;
    ensure!(errors.diagnostics().iter().any(|diagnostic| {
        diagnostic
            .message()
            .contains("the same schema source identity was imported more than once")
    }));
    ensure_eq!(names.name(SourceId::new(2)), None);

    Ok(())
}

#[cfg(unix)]
#[test]
fn root_schema_reads_the_file_opened_before_path_replacement() -> Result<(), Box<dyn Error>> {
    let compiler = new_compiler(SchemaCompileOptions::default())?;
    let context = compiler_context(&compiler)?;

    let directory = Directory::new()?;
    let path = directory.put("root.tschem", "entry struct Root {}")?;
    let replacement = directory.put("replacement.tschem", "?")?;
    let mut resolver = new_file_resolver(FileSchemaLimits::default(), &context)?;
    let mut root = read_root(&mut resolver, &path)?;
    rename(replacement, path)?;

    compile_schema(compiler, root.source(), &mut resolver, None).into_test_error()?;
    Ok(())
}

#[cfg(unix)]
#[test]
fn non_unicode_native_paths_are_preserved() -> Result<(), Box<dyn Error>> {
    let compiler = new_compiler(SchemaCompileOptions::default())?;
    let context = compiler_context(&compiler)?;

    let directory = Directory::new()?;
    let path = directory
        .path
        .join(OsString::from_vec(b"root-\xff.tschem".to_vec()));
    write(&path, "extern mod child; entry struct Root {}")?;
    directory.put("child.tschem", "struct Child {}")?;
    let mut resolver = new_file_resolver(FileSchemaLimits::default(), &context)?;
    let mut root = read_root(&mut resolver, &path)?;
    ensure_eq!(root.path(), path.canonicalize()?);
    compile_schema(compiler, root.source(), &mut resolver, None).into_test_error()?;
    ensure!(!format!("{root:?} {resolver:?}").contains("root-"));

    Ok(())
}

#[test]
fn maximum_default_file_size_remains_readable_with_default_execution_limits()
-> Result<(), Box<dyn Error>> {
    let directory = Directory::new()?;
    let bytes = vec![b' '; FileConfigInputLimits::default().max_file_bytes()];
    let path = directory.put("config.tconf", &bytes)?;
    let mut source = open_config_input(
        &path,
        FileConfigInputLimits::default(),
        bytes.len(),
        &new_budget(DEFAULT_TEST_EXECUTION)?,
    )?;
    let mut block = [0; 8192];
    let mut count = 0;
    loop {
        let read = read_source(&mut source, &mut block, SourceId::new(0), count)?;
        if read == 0 {
            break;
        }
        count += read;
    }
    ensure_eq!(count, bytes.len());

    Ok(())
}

#[cfg(unix)]
#[test]
fn native_candidate_length_equal_to_limit_succeeds() -> Result<(), Box<dyn Error>> {
    let compiler = new_compiler(SchemaCompileOptions::default())?;
    let context = compiler_context(&compiler)?;

    let directory = Directory::new()?;
    let path = directory.put("root.tschem", "entry struct Root {}")?;
    let child = directory.put("long_child_name.tschem", "struct Child {}")?;
    let mut params = defaults();
    params.max_path_bytes = child.canonicalize()?.as_os_str().len();
    let mut resolver = new_file_resolver(
        FileSchemaLimits::new(FileSchemaLimitParams {
            max_file_bytes: params.max_file_bytes,
            max_files: params.max_files,
            max_path_bytes: params.max_path_bytes,
            max_name_bytes: params.max_name_bytes,
        }),
        &context,
    )?;
    let _root = read_root(&mut resolver, &path)?;
    ensure!(
        resolve(
            &mut resolver,
            SchemaResolveRequest {
                from_identity: &0_u64.to_le_bytes(),
                module_name: "long_child_name",
                source_byte_limit: 100,
                context: &context
            }
        )
        .is_ok()
    );
    let mut params = defaults();
    params.max_path_bytes = child.canonicalize()?.as_os_str().len() - 1;
    let mut resolver = new_file_resolver(
        FileSchemaLimits::new(FileSchemaLimitParams {
            max_file_bytes: params.max_file_bytes,
            max_files: params.max_files,
            max_path_bytes: params.max_path_bytes,
            max_name_bytes: params.max_name_bytes,
        }),
        &context,
    )?;
    let _root = read_root(&mut resolver, &path)?;
    ensure!(matches!(
        resolve(
            &mut resolver,
            SchemaResolveRequest {
                from_identity: &0_u64.to_le_bytes(),
                module_name: "long_child_name",
                source_byte_limit: 100,
                context: &context
            }
        ),
        Err(ResolverError::ResourceLimitExceeded)
    ));

    Ok(())
}

#[test]
fn root_budget_failure_before_observer_does_not_invent_an_id() -> Result<(), Box<dyn Error>> {
    let directory = Directory::new()?;
    let path = directory.put("root.tschem", "entry struct Root {}")?;
    let mut limits = SchemaLimits::default();
    limits.set_max_schema_files(0);
    let compiler = new_compiler(SchemaCompileOptions::new(limits))?;
    let context = compiler_context(&compiler)?;
    let mut resolver = new_file_resolver(FileSchemaLimits::default(), &context)?;
    let names = new_source_names(&context)?;
    ensure_eq!(
        read_root(&mut resolver, &path)
            .err()
            .ok_or_else(|| io::Error::other("expected operation to fail"))?
            .kind(),
        HostErrorKind::ResourceLimitExceeded
    );
    ensure_eq!(names.name(SourceId::new(0)), None);
    ensure_eq!(names.failure(), None);

    Ok(())
}

#[test]
fn oversized_preferred_file_never_uses_valid_alternate() -> Result<(), Box<dyn Error>> {
    let compiler = new_compiler(SchemaCompileOptions::default())?;
    let context = compiler_context(&compiler)?;

    let directory = Directory::new()?;
    let path = directory.put("root.tschem", "extern mod child; entry struct Root {}")?;
    directory.put("child.tschem", vec![b' '; 101])?;
    directory.put("root/child.tschem", "struct Child {}")?;
    let mut resolver = new_file_resolver(FileSchemaLimits::default(), &context)?;
    let _root = read_root(&mut resolver, &path)?;
    ensure!(matches!(
        resolve(
            &mut resolver,
            SchemaResolveRequest {
                from_identity: &0_u64.to_le_bytes(),
                module_name: "child",
                source_byte_limit: 100,
                context: &context
            }
        ),
        Err(ResolverError::ResourceLimitExceeded)
    ));

    Ok(())
}

#[test]
fn config_handles_detect_growth_and_observe_truncation_after_metadata() -> Result<(), Box<dyn Error>>
{
    let directory = Directory::new()?;
    let path = directory.put("changing.tconf", b"{}")?;
    let budget = new_budget(DEFAULT_TEST_EXECUTION)?;
    let initial = budget.workspace_bytes();
    let mut input = open_config_input(&path, FileConfigInputLimits::default(), 2, &budget)?;
    write(&path, b"{} ")?;
    let mut bytes = [0; 7];
    ensure_eq!(read_source(&mut input, &mut bytes, SourceId::new(0), 0)?, 2);
    ensure_eq!(&bytes[..2], b"{}");
    let error = read_source(&mut input, &mut bytes, SourceId::new(0), 2)
        .err()
        .ok_or_else(|| io::Error::other("expected config source read to exceed its limit"))?;
    ensure_eq!(error.kind(), SourceReadErrorKind::ResourceLimitExceeded);
    ensure_eq!(error.offset(), Some(2));
    ensure_eq!(
        error
            .context()
            .ok_or_else(|| io::Error::other("read limit context is missing"))?
            .failure_point(),
        "source-byte-limit"
    );
    write(&path, b"{")?;
    for _ in 0..2 {
        let repeated_error = read_source(&mut input, &mut bytes, SourceId::new(0), 2)
            .err()
            .ok_or_else(|| {
                io::Error::other("config source must remain failed after exceeding its limit")
            })?;
        ensure_eq!(
            repeated_error.kind(),
            SourceReadErrorKind::InternalInvariantFailure
        );
        ensure_eq!(
            repeated_error
                .context()
                .ok_or_else(|| io::Error::other("config source state context is missing"))?
                .failure_point(),
            "stream-state-invariant"
        );
    }
    drop(input);
    ensure_eq!(budget.workspace_bytes(), initial);
    write(&path, b"{} ")?;
    let mut input = open_config_input(&path, FileConfigInputLimits::default(), 3, &budget)?;
    write(&path, b"{")?;
    ensure_eq!(read_source(&mut input, &mut bytes, SourceId::new(0), 0)?, 1);
    ensure_eq!(read_source(&mut input, &mut bytes, SourceId::new(0), 1)?, 0);
    write(&path, b"{} ")?;
    ensure_eq!(read_source(&mut input, &mut bytes, SourceId::new(0), 1)?, 0);
    ensure_eq!(read_source(&mut input, &mut [], SourceId::new(0), 1)?, 0);
    drop(input);
    ensure_eq!(budget.workspace_bytes(), initial);

    Ok(())
}

struct ResurrectingReader<'a> {
    calls: &'a Cell<usize>,
}

impl IoRead for ResurrectingReader<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        self.calls.set(self.calls.get() + 1);
        if self.calls.get() == 1 {
            return Ok(0);
        }
        let Some(first) = output.first_mut() else {
            return Err(io::Error::other("read buffer is empty"));
        };
        *first = b'x';
        Ok(1)
    }
}

struct FailingReader;

impl IoRead for FailingReader {
    fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "PRIVATE-READER-DETAIL",
        ))
    }
}

struct OverreportingReader;

impl IoRead for OverreportingReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        Ok(output.len() + 1)
    }
}

#[test]
fn reader_adapter_keeps_eof_permanent_and_empty_buffers_do_not_end_input()
-> Result<(), Box<dyn Error>> {
    let calls = Cell::new(0);
    let mut reader = ResurrectingReader { calls: &calls };
    let budget = new_budget(DEFAULT_TEST_EXECUTION)?;
    let mut source = new_read_source(&mut reader, &budget)?;
    ensure_eq!(read_source(&mut source, &mut [], SourceId::new(0), 0)?, 0);
    ensure_eq!(calls.get(), 0);
    ensure_eq!(
        read_source(&mut source, &mut [0; 1], SourceId::new(0), 0)?,
        0
    );
    let steps = budget.steps();
    ensure_eq!(
        read_source(&mut source, &mut [0; 1], SourceId::new(0), 0)?,
        0
    );
    ensure_eq!(calls.get(), 1);
    ensure_eq!(budget.steps(), steps);

    Ok(())
}

#[test]
fn reader_adapter_keeps_safe_offset_context_without_os_text() -> Result<(), Box<dyn Error>> {
    let mut reader = FailingReader;
    let budget = new_budget(DEFAULT_TEST_EXECUTION)?;
    let mut source = new_read_source(&mut reader, &budget)?;
    let error = read_source(&mut source, &mut [0; 5], SourceId::new(0), 0)
        .err()
        .ok_or_else(|| io::Error::other("expected host input to fail"))?;
    ensure_eq!(error.kind(), SourceReadErrorKind::HostFailure);
    ensure_eq!(error.offset(), Some(0));
    ensure_eq!(error.requested_bytes(), Some(5));
    ensure_eq!(
        error
            .context()
            .ok_or_else(|| io::Error::other("read context is missing"))?
            .failure_point(),
        "host-reader-call"
    );
    ensure!(!format!("{error:?} {error}").contains("PRIVATE-READER-DETAIL"));
    Ok(())
}

#[test]
fn reader_adapter_rejects_counts_beyond_the_supplied_buffer() -> Result<(), Box<dyn Error>> {
    let mut reader = OverreportingReader;
    let budget = new_budget(DEFAULT_TEST_EXECUTION)?;
    let mut source = new_read_source(&mut reader, &budget)?;
    let error = read_source(&mut source, &mut [0; 2], SourceId::new(0), 0)
        .err()
        .ok_or_else(|| io::Error::other("expected a read count invariant error"))?;
    ensure_eq!(error.kind(), SourceReadErrorKind::InternalInvariantFailure);
    ensure_eq!(error.offset(), Some(0));
    ensure_eq!(error.requested_bytes(), Some(2));
    ensure_eq!(
        error
            .context()
            .ok_or_else(|| io::Error::other("read count context is missing"))?
            .failure_point(),
        "read-count-exceeds-request"
    );
    Ok(())
}
