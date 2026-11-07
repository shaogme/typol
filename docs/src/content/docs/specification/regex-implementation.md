---
title: Regex implementation
description: Regex compilation, search semantics, resource limits, diagnostics, and runtime workspace.
sidebar:
  order: 6
---

## Quick reference

:::tip[Use this page when]
You are diagnosing `@regex` compilation, search behavior, or regex-specific resource failures.
:::

| Topic | Rule to remember |
| --- | --- |
| Matching | Typol searches for a match anywhere by default; use both `^` and `$` for whole-string matching. |
| Execution | Patterns compile to a Thompson NFA and run without backtracking; unsupported host-regex features remain unsupported. |
| Resource bounds | Compilation and matching use explicit limits; exhaustion returns a structured error and no partial match. |

This document describes Typol's current implementation of the Schema `@regex` annotation: its compiled representation, search execution, limits, diagnostics, and runtime workspace. The regex language and observable match semantics are defined by [Schema DSL §5.1](../schema-dsl/#51-regex-syntax-and-matching). This document records implementation behavior and boundaries without extending the DSL.

## 1. Scope

`typol-regex` is an independent `no_std + alloc` regex compiler and execution crate. It does not access Schema files, configuration environments, or host resources. Compile and match entry points require a `RegexExecutionBudget`; high-level Schema and Config callers create that budget from `RegexOptions` for each compile or runtime call. The generic `ExecutionBudget` accounts for non-Regex work only.

| Responsibility | Source location |
| --- | --- |
| Regex parser and AST | `crates/typol-regex/src/parser.rs`, `parser/character_class.rs` |
| Unicode scalar character sets | `crates/typol-regex/src/char_class.rs` |
| Thompson NFA compiler and immutable program | `crates/typol-regex/src/compiler.rs` |
| NFA search and reusable workspace | `crates/typol-regex/src/matcher.rs` |
| Structured errors and pattern spans | `crates/typol-regex/src/error.rs` |
| Schema annotation compilation and regex collection | `crates/typol-schema/src/regex.rs`, `crates/typol-semantic/src/regex.rs` |
| Semantic executor and constraint checks | `crates/typol-semantic/src/engine/regex.rs`, `engine/rules.rs` |
| Regex resource defaults | `crates/typol-policy/src/lib.rs` |

## 2. Schema compilation

The Schema compiler accepts `@regex` only on `string` or `option<string>`. An optional value of `none` skips matching; a present string is searched.

The Schema lexer decodes the annotation string using DSL rules, then passes the decoded UTF-8 pattern to `RegexCompiler`. The regex compiler reads Unicode scalar values into an internal AST, then emits an owned Thompson NFA. On success, the NFA and annotation `SourceSpan` are stored in `CompiledRegexConstraints`. Runtime validation never reparses or recompiles a pattern.

Schema compilation accumulates attempted pattern bytes across every valid-target `@regex`, including syntactically invalid patterns. Public `RegexCounts` counts only successfully stored programs. Remaining Schema-wide instruction and range budgets are passed into the regex compiler while parsing and constructing the NFA, so an oversized program is rejected before it is fully built.

The Schema compiler indexes programs by annotation source span. Syntax errors map to `REGEX_SYNTAX_ERROR` at the annotation; resource exhaustion maps to a resource diagnostic and allocation failure to an allocation diagnostic. Low-level regex errors contain only an error category and a span within the pattern; they do not store the pattern text in the error or its debug output.

## 3. Parsing and character sets

The parser uses explicit frames for groups so group nesting does not grow the host call stack with input depth. A `PatternSpan` uses half-open UTF-8 byte offsets within the decoded pattern, not offsets in the Schema source.

The parser implements literals, groups, alternation, concatenation, anchors, quantifiers, character classes, and escapes listed by the Schema DSL. It rejects empty alternatives, empty character classes, reversed ranges, ranges with non-literal endpoints, repeated quantifiers, and unsupported escapes. It does not accept additional syntax from host regex libraries, such as look-around, backreferences, or Unicode property escapes.

Character classes compile to sorted, merged, closed intervals of Unicode scalar values. Complements are calculated within the scalar-value domain and skip the surrogate range `U+D800..U+DFFF`. `\d`, `\w`, and `\s` use the DSL's ASCII sets; uppercase variants use their complement within the scalar-value domain. A dot compiles to all scalar values except LF. Matching performs no normalization, case folding, or locale processing.

## 4. Thompson NFA and search semantics

The compiled program consists of an instruction array and a start instruction index. Instructions consume one character set, branch, jump, assert start/end, or represent a match. Groups affect parsing precedence but do not capture text. Quantifiers expand by cloning finite subprograms and adding branch/loop instructions; finite repetition bounds are at most 255.

The executor scans input Unicode scalar values. At every input position it adds the pattern's start state to the active set, implementing search rather than whole-string matching by default. `^` succeeds only at input start and `$` only at input end; they are not multiline anchors. Empty patterns and expressions that can match an empty string may succeed at any input position. Callers use both anchors when whole-string matching is required.

NFA states advance as deduplicated sets, with no backtracking. Alternation and repetition therefore do not create path-based exponential backtracking. Character-set membership uses binary search over sorted intervals. Each match has an independent `max_search_steps` limit counting NFA states processed by that search. Cumulative Regex compilation and matching work shares `max_steps`; exhausting either limit returns a structured resource error, not a partial match result.

## 5. Resource limits

`RegexLimits::default()` supplies structural bounds. Callers configure them separately from the execution quotas in `RegexExecutionLimits`, both of which are composed by `RegexOptions`. These defaults are implementation policies, not permanent DSL semantics.

| Limit | Default | Scope |
| --- | ---: | --- |
| `max_regex_pattern_bytes` | 8,192 | UTF-8 bytes in one decoded pattern |
| `max_regex_total_pattern_bytes` | 65,536 | Attempted pattern bytes for valid-target annotations in one Schema, including invalid patterns |
| `max_regex_count` | 16 | Successfully stored programs in one Schema |
| `max_regex_nesting_depth` | 64 | Group nesting depth of one pattern |
| `max_regex_instructions_per_pattern` | 4,096 | NFA instructions in one compiled program |
| `max_regex_ranges_per_pattern` | 4,096 | Character intervals in one program, including quantifier expansion |
| `max_regex_instructions_total` | 16,384 | Total instructions across programs in one Schema |
| `max_regex_ranges_total` | 16,384 | Total character intervals across programs in one Schema |
| `max_regex_scratch_states` | 4,096 | State slots in one execution workspace |

Regex execution defaults from `RegexExecutionLimits::default()` are:

| Limit | Default | Scope |
| --- | ---: | --- |
| `max_steps` | 67,108,864 | Cumulative explicitly metered Regex compiler and matcher work in one call |
| `max_search_steps` | 67,108,864 | NFA states processed by one search |
| `max_workspace_bytes` | 67,108,864 | Live Regex-owned capacity in one call |

Pattern size counts UTF-8 bytes, not characters. Attempted pattern bytes bound compilation work and are not refunded for invalid syntax. Public pattern-byte, range, and instruction statistics include only saved programs, so an invalid pattern does not affect artifact statistics. Quantifier expansion increases instruction and range use. Both per-program and remaining Schema-wide limits constrain parsing and program construction; exceeding either fails compilation or execution. A Schema compile shares one Regex budget across pattern compilation and default-value Regex checks. A Config parse or external-value validation creates its own shared Regex budget even when it reuses a caller's generic execution budget. Normalize-only operations do not create or consume a Regex budget.

Containers for AST, NFA, character sets, and workspaces use fallible charged vectors and are charged to the Regex budget for steps and live workspace. The regex budget's fixed bookkeeping is excluded from its workspace limit. Capacity exhaustion is a resource error; allocator rejection is an allocation error. Errors do not include the pattern or input.

## 6. Workspace lifecycle and errors

`RegexScratch` holds active states, next states, a traversal stack, and deduplication marks for one NFA search. Initial use allocates for the program instruction count. Later growth occurs before matching under the current Regex budget; matching itself does not grow the workspace. A growth failure keeps its specific error category and does not proactively discard existing storage. Allocation failure does not exhaust the Regex budget, so the previous workspace remains usable. Insufficient Regex workspace terminates that Regex budget. If the scratch limit is lowered, cached storage above the limit is released first.

The semantic `RegexExecutor` reuses and grows scratch across multiple constraints. When called with a new Regex budget it discards the old workspace so capacity from the previous call is not charged to the new one. For an empty optional value, it first clears stale or over-limit workspace associated with a changed budget, then passes without creating scratch for the new budget.

Regex failures are syntax errors, resource-limit errors, or allocation errors. Match failures report resource exhaustion only. The Schema diagnostic layer maps the low-level category to a stable diagnostic code and annotation location. A syntax error also carries its in-pattern `PatternSpan` for direct `typol-regex` callers that need finer locations.

## 7. Verification coverage

`crates/typol-regex/tests/regex_behavior.rs` covers search and anchors, alternation and repetition, escapes, ASCII shorthand classes, Unicode scalars and surrogate gaps, error spans, and limit boundaries. Compiler unit tests cover dot behavior with three or two available intervals. `compilation_budget.rs`, `compilation_allocation.rs`, and `allocation_failure.rs` cover shared budgets, bounded stacks, allocation failures, and cleanup. Schema tests `regex_constraint_collection.rs` and `regex_constraint_resources.rs` cover imports, attempted-byte limits, artifact statistics, and diagnostic priority. Semantic tests `regex_execution.rs`, `regex_budget.rs`, and `regex_allocation.rs` cover workspace growth, growth failure, budget changes, and limit-based release.
