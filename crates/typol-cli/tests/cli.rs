use std::{
    env::temp_dir,
    error::Error,
    ffi::OsString,
    fmt::Debug,
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Output, id},
};

const CLI_PATH: &str = env!("CARGO_BIN_EXE_typol");

fn example(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/cli")
        .join(name)
}

fn argument(value: &str) -> OsString {
    OsString::from(value)
}

fn example_argument(name: &str) -> OsString {
    example(name).into_os_string()
}

fn run_cli(arguments: Vec<OsString>) -> io::Result<Output> {
    Command::new(CLI_PATH).args(arguments).output()
}

fn run_cli_with_variable(arguments: Vec<OsString>, name: &str, value: &str) -> io::Result<Output> {
    Command::new(CLI_PATH)
        .args(arguments)
        .env(name, value)
        .output()
}

fn run_cli_with_variables(
    arguments: Vec<OsString>,
    variables: &[(&str, &str)],
) -> io::Result<Output> {
    let mut command = Command::new(CLI_PATH);
    command.args(arguments).env_clear();
    for (name, value) in variables {
        command.env(name, value);
    }
    command.output()
}

fn config_arguments(action: &str, schema: &str, config: &str, pretty: bool) -> Vec<OsString> {
    config_arguments_for(action, schema, config, "empty", pretty)
}

fn config_arguments_for(
    action: &str,
    schema: &str,
    config: &str,
    environment: &str,
    pretty: bool,
) -> Vec<OsString> {
    let mut arguments = vec![
        argument("config"),
        argument(action),
        argument("--schema"),
        example_argument(schema),
        argument("--environment"),
        argument(environment),
    ];
    if pretty {
        arguments.push(argument("--pretty"));
    }
    arguments.push(example_argument(config));
    arguments
}

fn ensure(condition: bool, message: &str) -> Result<(), Box<dyn Error>> {
    if condition {
        Ok(())
    } else {
        Err(io::Error::other(message).into())
    }
}

fn ensure_equal<T: Debug + PartialEq>(
    actual: T,
    expected: T,
    message: &str,
) -> Result<(), Box<dyn Error>> {
    if actual == expected {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "{message}: actual value {actual:?}, expected value {expected:?}"
        ))
        .into())
    }
}

fn assert_localized_schema_diagnostic(
    source: &str,
    expected: &str,
    case: &str,
) -> Result<(), Box<dyn Error>> {
    let path = temp_dir().join(format!("typol-localized-{case}-{}.tschem", id()));
    fs::write(&path, source)?;
    let output = run_cli(vec![
        argument("--language=zh-CN"),
        argument("schema"),
        argument("check"),
        path.clone().into_os_string(),
    ])?;
    fs::remove_file(&path)?;
    ensure_equal(output.status.code(), Some(1), "invalid schema exit code")?;
    ensure(
        String::from_utf8_lossy(&output.stderr).contains(expected),
        "schema diagnostic must use the selected language",
    )
}

#[test]
fn schema_and_config_checks_succeed_without_stdout() -> Result<(), Box<dyn Error>> {
    let schema = run_cli(vec![
        argument("schema"),
        argument("check"),
        example_argument("cli_demo.tschem"),
    ])?;
    ensure_equal(schema.status.code(), Some(0), "schema check exit code")?;
    ensure(
        schema.stdout.is_empty(),
        "schema check must not write to stdout",
    )?;

    let config = run_cli(config_arguments(
        "check",
        "cli_demo.tschem",
        "cli_demo.tconf",
        false,
    ))?;
    ensure_equal(config.status.code(), Some(0), "config check exit code")?;
    ensure(
        config.stdout.is_empty(),
        "config check must not write to stdout",
    )?;
    Ok(())
}

#[test]
fn export_ir_writes_to_stdout() -> Result<(), Box<dyn Error>> {
    let output = run_cli(config_arguments(
        "export-ir",
        "cli_demo.tschem",
        "cli_demo.tconf",
        false,
    ))?;
    ensure_equal(output.status.code(), Some(0), "IR export exit code")?;
    ensure(!output.stdout.is_empty(), "IR must be written to stdout")?;
    ensure(
        output.stdout.starts_with(b"{"),
        "IR must start with a JSON object",
    )?;
    ensure(
        output.stdout.ends_with(b"\n"),
        "IR output must end with a newline",
    )?;
    ensure(
        output.stderr.is_empty(),
        "successful export must not write to stderr",
    )?;
    Ok(())
}

#[test]
fn secret_rejection_prevents_ir_from_reaching_stdout() -> Result<(), Box<dyn Error>> {
    let mut arguments = config_arguments("export-ir", "cli_demo.tschem", "cli_demo.tconf", false);
    arguments.insert(arguments.len() - 1, argument("--reject-secrets"));
    let output = run_cli(arguments)?;

    ensure_equal(
        output.status.code(),
        Some(3),
        "secret IR rejection exit code",
    )?;
    ensure(
        output.stdout.is_empty(),
        "rejected IR must not reach stdout",
    )?;
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    ensure(
        diagnostic.contains("secret value") && !diagnostic.contains("demo-secret-value"),
        "secret rejection must report a safe diagnostic",
    )?;

    let mut invalid_arguments =
        config_arguments("check", "cli_demo.tschem", "cli_demo.tconf", false);
    invalid_arguments.insert(invalid_arguments.len() - 1, argument("--reject-secrets"));
    let invalid_option = run_cli(invalid_arguments)?;
    ensure_equal(
        invalid_option.status.code(),
        Some(2),
        "invalid secret option exit code",
    )?;

    let schema_path = temp_dir().join(format!("typol-secret-enum-{}.tschem", id()));
    let config_path = temp_dir().join(format!("typol-secret-enum-{}.tconf", id()));
    fs::write(
        &schema_path,
        "enum Access { Token(string @secret), Ready } entry struct Root { access: Access }",
    )?;
    fs::write(&config_path, "{ access: .Token(\"enum-secret-value\") }")?;
    let enum_output = run_cli(vec![
        argument("config"),
        argument("export-ir"),
        argument("--schema"),
        schema_path.clone().into_os_string(),
        argument("--environment"),
        argument("empty"),
        argument("--reject-secrets"),
        config_path.clone().into_os_string(),
    ])?;
    fs::remove_file(&schema_path)?;
    fs::remove_file(&config_path)?;
    ensure_equal(
        enum_output.status.code(),
        Some(3),
        "secret enum rejection exit code",
    )?;
    ensure(
        enum_output.stdout.is_empty(),
        "secret enum IR must not reach stdout",
    )?;
    ensure(
        !String::from_utf8_lossy(&enum_output.stderr).contains("enum-secret-value"),
        "secret enum rejection must not expose its payload",
    )?;
    Ok(())
}

#[test]
fn environment_mode_selects_process_or_fallback_value() -> Result<(), Box<dyn Error>> {
    let empty = run_cli_with_variable(
        config_arguments_for(
            "export-ir",
            "cli_demo.tschem",
            "cli_demo.tconf",
            "empty",
            false,
        ),
        "TYPOL_CLI_DEMO_KEY",
        "environment-secret-value",
    )?;
    ensure_equal(
        empty.status.code(),
        Some(0),
        "empty environment mode exit code",
    )?;

    let inherited = run_cli_with_variable(
        config_arguments_for(
            "export-ir",
            "cli_demo.tschem",
            "cli_demo.tconf",
            "inherit",
            false,
        ),
        "TYPOL_CLI_DEMO_KEY",
        "environment-secret-value",
    )?;
    ensure_equal(
        inherited.status.code(),
        Some(0),
        "inherit environment mode exit code",
    )?;
    ensure(
        empty.stdout != inherited.stdout,
        "empty and inherit must export their respective selected environment values",
    )?;
    Ok(())
}

#[test]
fn usage_validation_and_secret_diagnostics_use_contract_exit_codes() -> Result<(), Box<dyn Error>> {
    let help = run_cli(vec![
        argument("schema"),
        argument("check"),
        argument("--help"),
    ])?;
    ensure_equal(
        help.status.code(),
        Some(0),
        "schema subcommand help exit code",
    )?;
    ensure(
        !help.stdout.is_empty(),
        "help text must be written to stdout",
    )?;

    let usage = run_cli(vec![argument("config"), argument("unknown")])?;
    ensure_equal(usage.status.code(), Some(2), "usage error exit code")?;
    ensure(
        usage.stdout.is_empty(),
        "usage errors must not write to stdout",
    )?;

    let invalid_config = run_cli(config_arguments(
        "check",
        "cli_demo.tschem",
        "cli_demo-invalid.tconf",
        false,
    ))?;
    ensure_equal(
        invalid_config.status.code(),
        Some(1),
        "config validation failure exit code",
    )?;
    let diagnostic = String::from_utf8_lossy(&invalid_config.stderr);
    ensure(
        !diagnostic.contains("short-secret"),
        "secret validation diagnostics must not reveal the original value",
    )?;

    let missing_schema = run_cli(vec![
        argument("schema"),
        argument("check"),
        example_argument("missing-schema.tschem"),
    ])?;
    ensure_equal(
        missing_schema.status.code(),
        Some(3),
        "file read failure exit code",
    )?;
    Ok(())
}

#[test]
fn language_option_localizes_help_and_usage_errors() -> Result<(), Box<dyn Error>> {
    let english = run_cli(vec![
        argument("--language"),
        argument("en"),
        argument("--help"),
    ])?;
    ensure_equal(english.status.code(), Some(0), "English help exit code")?;
    ensure(
        String::from_utf8_lossy(&english.stdout).starts_with("Usage:"),
        "English help must use the English catalog",
    )?;
    ensure(
        english.stdout.ends_with(b"\n"),
        "help output must end with a newline",
    )?;

    let chinese_help = run_cli(vec![argument("--language=zh-CN"), argument("--help")])?;
    ensure_equal(
        chinese_help.status.code(),
        Some(0),
        "Chinese help exit code",
    )?;
    ensure(
        String::from_utf8_lossy(&chinese_help.stdout).starts_with("用法："),
        "the equals form must select Simplified Chinese",
    )?;

    let detected = run_cli_with_variable(vec![argument("--help")], "TYPOL_LANG", "zh_CN.UTF-8")?;
    ensure(
        String::from_utf8_lossy(&detected.stdout).starts_with("用法："),
        "TYPOL_LANG must accept common system locale names",
    )?;
    let traditional = run_cli_with_variable(vec![argument("--help")], "TYPOL_LANG", "zh-Hant")?;
    ensure(
        String::from_utf8_lossy(&traditional.stdout).starts_with("用法："),
        "all Chinese locale tags must select the Simplified Chinese catalog",
    )?;
    let next_candidate = run_cli_with_variables(
        vec![argument("--help")],
        &[
            ("TYPOL_LANG", "fr_FR"),
            ("LC_ALL", "zh_CN.UTF-8"),
            ("LC_MESSAGES", "C"),
            ("LANG", "C"),
        ],
    )?;
    ensure(
        String::from_utf8_lossy(&next_candidate.stdout).starts_with("用法："),
        "locale detection must continue after an unsupported preferred value",
    )?;
    let regional_english = run_cli_with_variable(vec![argument("--help")], "TYPOL_LANG", "en-150")?;
    ensure(
        String::from_utf8_lossy(&regional_english.stdout).starts_with("Usage:"),
        "regional English tags must resolve through catalog fallback",
    )?;

    for unsupported_language in ["zh-Hant", "zh-TW"] {
        let invalid = run_cli_with_variable(
            vec![
                argument(&format!("--language={unsupported_language}")),
                argument("--help"),
            ],
            "TYPOL_LANG",
            "en_US.UTF-8",
        )?;
        ensure_equal(
            invalid.status.code(),
            Some(2),
            "unsupported language option exit code",
        )?;
        ensure(
            String::from_utf8_lossy(&invalid.stderr).contains("--language must be en or zh-CN."),
            "CLI language options must reject locale variants not listed in help",
        )?;
    }

    let chinese = run_cli(vec![
        argument("--language"),
        argument("zh-CN"),
        argument("config"),
        argument("check"),
        argument("--wat"),
    ])?;
    ensure_equal(chinese.status.code(), Some(2), "Chinese usage exit code")?;
    let diagnostic = String::from_utf8_lossy(&chinese.stderr);
    ensure(
        diagnostic.contains("命令行错误：") && diagnostic.contains("未知的 config 选项。"),
        "Chinese usage errors must use the Chinese catalog",
    )?;
    ensure(
        diagnostic.contains("用法："),
        "Chinese usage errors must include Chinese help",
    )?;
    Ok(())
}

#[test]
fn language_option_localizes_schema_and_configuration_diagnostics() -> Result<(), Box<dyn Error>> {
    let mut schema_arguments = vec![
        argument("--language=zh-CN"),
        argument("schema"),
        argument("check"),
    ];
    let schema_path = temp_dir().join(format!("typol-localized-schema-{}.tschem", id()));
    fs::write(&schema_path, "struct Root {}")?;
    schema_arguments.push(schema_path.clone().into_os_string());
    let schema = run_cli(schema_arguments)?;
    fs::remove_file(&schema_path)?;
    ensure_equal(schema.status.code(), Some(1), "invalid schema exit code")?;
    let schema_text = String::from_utf8_lossy(&schema.stderr);
    ensure(
        schema_text.contains("Typol 诊断详情：")
            && schema_text.contains("Schema 编译失败")
            && schema_text.contains("根 Schema 必须声明 entry 结构体"),
        "schema diagnostic details must use the selected language",
    )?;

    let mut config = vec![argument("--language"), argument("zh-CN")];
    config.extend(config_arguments(
        "check",
        "cli_demo.tschem",
        "cli_demo-invalid.tconf",
        false,
    ));
    let config = run_cli(config)?;
    ensure_equal(config.status.code(), Some(1), "invalid config exit code")?;
    let config_text = String::from_utf8_lossy(&config.stderr);
    ensure(
        config_text.contains("Typol 操作失败")
            && config_text.contains("值已隐藏")
            && config_text.contains("约束定义于此处"),
        "configuration diagnostic details must use the selected language",
    )?;
    ensure(
        !config_text.contains("value is hidden")
            && !config_text.contains("constraint defined here")
            && !config_text.contains("short-secret"),
        "localized output must preserve secret redaction",
    )?;

    assert_localized_schema_diagnostic("entry struct Root {} €", "来源包含无效字符", "lexer")?;
    assert_localized_schema_diagnostic(
        "entry struct Root { value: bool @min(1), }",
        "数值边界注解仅适用于数值类型",
        "annotation",
    )?;
    assert_localized_schema_diagnostic(
        "entry struct Root { value: bool = 1, }",
        "数值默认值与目标类型不匹配",
        "default",
    )?;
    assert_localized_schema_diagnostic(
        "entry struct First {} entry struct Second {}",
        "根 Schema 只能声明一个 entry 结构体",
        "duplicate-entry",
    )?;
    assert_localized_schema_diagnostic(
        "entry struct Root { value: string, value: bool, }",
        "同一结构体中不能重复字段名称",
        "duplicate-field",
    )?;
    assert_localized_schema_diagnostic(
        "entry struct Root { value: string @regex(\"(\"), }",
        "正则表达式语法无效",
        "regex-syntax",
    )?;
    assert_localized_schema_diagnostic(
        "entry struct Root { value: u8 } @assert(self.value < 256, \"value must fit\")",
        "整数字面量超出目标类型的范围",
        "numeric-range",
    )?;
    assert_localized_schema_diagnostic(
        "type Nested = option<option<string>>; entry struct Root { value: Nested, }",
        "完全展开后不支持嵌套 Option",
        "nested-option",
    )?;
    Ok(())
}
