---
title: Regex implementation
description: Regex compilation, search semantics, resource limits, and runtime workspace.
sidebar:
  order: 6
---

## Quick reference

:::tip[Use this page when]
You are diagnosing `@regex` compilation, search behavior, or regex-specific
resource failures.
:::

- **Matching:** Typol searches for a match anywhere; use `^` and `$` for a whole string.
- **Execution:** Patterns use a Thompson NFA, without backtracking or
  host-regex extensions.
- **Limits:** Compilation and matching have explicit quotas and return errors
  on exhaustion.

This page describes how Typol currently compiles and runs the Schema `@regex` annotation.
The language and observable matching rules are defined by
[Schema DSL §5.1](../schema-dsl/#51-regex-syntax-and-matching). The details here describe
the implementation and its limits; they do not extend the DSL.

## 1. Scope and source map

`typol-regex` is an independent `no_std + alloc` compiler and matcher. It does not read
Schema files, configuration environments, or host resources. Its compile and match entry
points require a `RegexExecutionBudget`; the generic `ExecutionBudget` meters other work.

Schema compilation shares one Regex budget across pattern compilation and regex checks on
Schema defaults. Each configuration parse and external-value validation also shares one
Regex budget across its regex checks. Normalize-only operations do not create or consume
that budget.

- Parser and AST: `crates/typol-regex/src/parser.rs` and `parser/character_class.rs`
- Character sets: `crates/typol-regex/src/char_class.rs`
- NFA compiler: `crates/typol-regex/src/compiler.rs`
- Matcher and scratch workspace: `crates/typol-regex/src/matcher.rs`
- Errors and pattern spans: `crates/typol-regex/src/error.rs`
- Schema and semantic integration: `crates/typol-schema/src/regex.rs`,
  `crates/typol-semantic/src/regex.rs`, and `crates/typol-semantic/src/engine/regex.rs`
- Resource defaults: `crates/typol-policy/src/lib.rs`

## 2. Schema compilation

The Schema compiler accepts `@regex` on `string` and `option<string>`. A `none` optional
value passes without matching; a present string is searched. The Schema lexer first
decodes the annotation string, then the regex compiler parses that decoded UTF-8 pattern
into an AST and emits an owned Thompson NFA.

The compiled program and its annotation `SourceSpan` are stored in
`CompiledRegexConstraints`. Runtime validation finds a program by that source span; it
does not parse or compile the pattern again. The compiler uses explicit frames for nested
groups, and it checks instruction and character-range ceilings as it builds the program.

For each valid-target annotation that passes the program-count check, attempted pattern
bytes are added before syntax compilation. A syntax-invalid pattern therefore consumes
the attempted-byte allowance but is not included in the public `RegexCounts` artifact
statistics. The count ceiling applies to successfully stored programs and is checked
before byte accounting for the next annotation.

Syntax failures map to `REGEX_SYNTAX_ERROR` at the `@regex` annotation. Resource,
allocation, capacity, and internal failures retain distinct diagnostic categories. A
low-level syntax error stores its kind and a `PatternSpan` within the decoded pattern;
low-level errors do not copy the pattern or the value being matched.

## 3. Parsing and character sets

Parser spans are half-open UTF-8 byte offsets into the decoded pattern, not offsets into
the Schema source. Explicit parser frames keep group nesting from growing the host call
stack with input depth. The supported syntax is listed in
[Schema DSL §5.1](../schema-dsl/#51-regex-syntax-and-matching).

The parser rejects empty alternatives and character classes, reversed ranges, ranges
whose endpoints are not literal characters, repeated quantifiers, and unsupported
escapes. Look-around, backreferences, and Unicode property escapes are examples of
syntax provided by some host regex libraries that Typol does not accept.

Character classes become sorted, merged, closed intervals over Unicode scalar values.
Complements stay within that domain and skip the surrogate range `U+D800..U+DFFF`.
The `\d`, `\w`, and `\s` classes use the DSL's ASCII sets; uppercase forms use their
complements. A dot matches every scalar value except LF. Matching performs no Unicode
normalization, case folding, or locale processing.

## 4. Thompson NFA and search

The compiled program contains an instruction array and a start index. Instructions
consume a character set, branch, jump, assert the start or end of input, or mark a match.
Groups affect precedence but do not capture text. Bounded repetitions compile repeated
copies of their finite subprogram; optional and unbounded repetitions use branches and
loops. Finite bounds cannot exceed 255.

The matcher scans input by Unicode scalar value and adds the pattern's start state at
each input position. This implements substring search by default. `^` succeeds only at
the beginning of the whole input, and `$` only at its end; neither is a multiline anchor.
Empty patterns and other expressions that can match an empty string may match at any
input position. Use both anchors when the whole string must match.

The matcher advances deduplicated sets of NFA states and does not backtrack. Alternation
and repetition therefore do not cause path-based exponential backtracking. Membership
checks use binary search over sorted character intervals. `max_search_steps` counts NFA
states processed by one search; `max_steps` meters cumulative Regex compiler and matcher
work. Exhaustion returns an error instead of a partial match result.

## 5. Resource limits

`RegexLimits::default()` supplies structural limits. `RegexExecutionLimits::default()`
supplies runtime quotas. Callers can configure both through `RegexOptions`; these defaults
are implementation policies and are not permanent DSL guarantees.

Structural defaults:

- `max_regex_pattern_bytes`: 8,192 UTF-8 bytes per decoded pattern.
- `max_regex_total_pattern_bytes`: 65,536 attempted bytes per Schema.
- `max_regex_count`: 16 successfully stored programs per Schema.
- `max_regex_nesting_depth`: 64 nested groups per pattern.
- `max_regex_instructions_per_pattern`: 4,096 NFA instructions per program.
- `max_regex_ranges_per_pattern`: 4,096 character intervals per program.
- `max_regex_instructions_total`: 16,384 instructions across a Schema.
- `max_regex_ranges_total`: 16,384 character intervals across a Schema.
- `max_regex_scratch_states`: 4,096 state slots in one workspace.

Runtime defaults:

- `max_steps`: 67,108,864 cumulative, explicitly metered Regex steps per call.
- `max_search_steps`: 67,108,864 NFA states processed by one search.
- `max_workspace_bytes`: 67,108,864 bytes of live, charged Regex-owned capacity.

Pattern sizes count UTF-8 bytes, not Unicode characters. Invalid syntax still consumes
attempted bytes when it reaches byte accounting, while public byte, range, and instruction
statistics count only saved programs. Quantifier expansion increases instruction and
range use. Per-pattern ceilings and the remaining Schema-wide instruction and range
ceilings constrain parsing and NFA construction.

The Regex budget charges explicitly metered work and live capacity for ASTs, NFA programs,
character sets, and workspaces. Its fixed bookkeeping allocation is excluded from
`max_workspace_bytes`; the quota also does not represent total process memory. Logical
limit failures, allocator refusal, capacity failures, and internal invariant failures
remain distinct error categories.

## 6. Workspace lifecycle and errors

`RegexScratch` holds active states, next states, a traversal stack, and deduplication
marks. It is initialized for a program's instruction count and can grow before a search
under the same Regex budget; the search itself does not grow the workspace.

A failed growth preserves its specific error category and the existing workspace.
Allocation failure alone does not exhaust the Regex budget. A lowered scratch-state
ceiling causes the executor to release cached storage above the new ceiling before reuse.

The semantic `RegexExecutor` reuses scratch across constraints in one call. When the next
call has a different Regex budget, it drops the old scratch so its capacity is not charged
to the new budget. For an empty optional value, it performs that budget and limit cleanup,
then passes without creating scratch for the new budget.

Execution-step, search-step, or charged-workspace exhaustion marks the Regex budget as
failed. Structural limits such as the scratch-state ceiling are reported at the check
that detects them. Higher-level Schema diagnostics point to the annotation and may
include the usual source context; low-level errors retain metadata rather than copying
the decoded pattern or input value.

## 7. Verification coverage

`crates/typol-regex/tests/regex_behavior.rs` covers search, anchors, quantifiers, escapes,
ASCII shorthand classes, Unicode scalars, error spans, and limit boundaries. The regex
crate's compiler and budget tests also cover range accounting and allocation failures.

Schema integration is covered by `regex_constraint_collection.rs`,
`regex_constraint_resources.rs`, and `regex_constraint_diagnostics.rs` in
`crates/typol-schema/tests`. Semantic execution, budget, and allocation tests are in
`crates/typol-semantic/tests/regex_execution.rs`, `regex_budget.rs`, and
`regex_allocation.rs`.
