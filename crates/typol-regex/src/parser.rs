mod character_class;

use core::{fmt, mem::replace};

use opeo::{OResult, Out, ResultOutExt};
use typol_core::{ChargedVec, ExecutionBudget};
use typol_policy::RegexLimits;

use crate::{
    PatternSpan, RegexCompileError, RegexErrorContext, RegexResource, RegexSyntaxError,
    RegexSyntaxErrorKind,
    char_class::{CharClass, CharClassBuilder},
    error::{
        AST_NODE_MISSING, GROUP_FRAME_CURRENT_MISSING, GROUP_FRAME_POP_MISSING,
        PARSER_ALTERNATION_BRANCH_MISSING, PARSER_FIRST_CHILD_MISSING, PARSER_NODE_MISSING,
        PARSER_SINGLE_CHILD_MISSING,
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct NodeId(usize);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Repetition {
    ZeroOrMore,
    OneOrMore,
    ZeroOrOne,
    Bounded { min: u16, max: Option<u16> },
}

pub(crate) enum NodeKind {
    Empty,
    Literal(char),
    CharacterClass(CharClass),
    Start,
    End,
    Group(NodeId),
    Concatenation(ChargedVec<NodeId>),
    Alternation(ChargedVec<NodeId>),
    Repetition { node: NodeId, kind: Repetition },
}

pub(crate) struct Node {
    pub(crate) kind: NodeKind,
    pub(crate) span: PatternSpan,
}

pub(crate) struct Ast {
    nodes: ChargedVec<Node>,
    root: NodeId,
}

pub(super) enum EscapeValue {
    Literal(char),
    Class(CharClass),
}

struct GroupFrame {
    start: usize,
    children: ChargedVec<NodeId>,
    branches: ChargedVec<NodeId>,
    after_bar: bool,
}

struct GroupFrames {
    values: ChargedVec<GroupFrame>,
}

pub(crate) struct Parser<'a> {
    pattern: &'a str,
    position: usize,
    limits: &'a RegexLimits,
    max_ranges: usize,
    range_count: usize,
    nodes: ChargedVec<Node>,
    execution: &'a ExecutionBudget,
}

impl Ast {
    pub(crate) fn root(&self) -> NodeId {
        self.root
    }

    pub(crate) fn node<'slot>(
        &self,
        id: NodeId,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, &Node> {
        self.nodes
            .get(id.0)
            .ok_or(RegexCompileError::InternalInvariantFailure(
                RegexErrorContext::new(AST_NODE_MISSING),
            ))
            .store_err_into(out.reborrow())
    }
}

impl fmt::Debug for Ast {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Ast")
            .field("root", &self.root)
            .field("node_count", &self.nodes.len())
            .finish()
    }
}

impl Parser<'_> {
    pub(crate) fn parse_with_range_limit<'slot>(
        pattern: &str,
        limits: &RegexLimits,
        execution: &ExecutionBudget,
        max_ranges: usize,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, Ast> {
        if pattern.len() > limits.max_regex_pattern_bytes() {
            return Err(out.fail(RegexCompileError::ResourceLimitExceeded(
                RegexResource::PatternBytes,
            )));
        }

        let nodes = ChargedVec::new(Some(execution), out.reborrow())?;
        let mut parser = Parser {
            pattern,
            position: 0,
            limits,
            max_ranges: max_ranges.min(limits.max_regex_ranges_per_pattern()),
            range_count: 0,
            nodes,
            execution,
        };
        let root = parser.parse_groups(out.reborrow())?;
        if parser.position < pattern.len() {
            let end = parser.position
                + parser
                    .current_char(out.reborrow())?
                    .map_or(0, char::len_utf8);
            return Err(out.fail(parser.syntax(
                RegexSyntaxErrorKind::UnexpectedToken,
                parser.position,
                end,
            )));
        }
        Ok(Ast {
            nodes: parser.nodes,
            root,
        })
    }

    fn parse_groups<'slot>(
        &mut self,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, NodeId> {
        let mut frames = GroupFrames::new(self.execution, out.reborrow())?;
        let first_frame = GroupFrame::new(0, self.execution, out.reborrow())?;
        frames.push(first_frame, out.reborrow())?;
        loop {
            self.execution.consume(1, out.reborrow())?;
            match self.current_char(out.reborrow())? {
                Some('(') => {
                    if frames.len() > self.limits.max_regex_nesting_depth() {
                        return Err(out.fail(RegexCompileError::ResourceLimitExceeded(
                            RegexResource::NestingDepth,
                        )));
                    }
                    let start = self.position;
                    self.advance_char(out.reborrow())?;
                    let frame = GroupFrame::new(start, self.execution, out.reborrow())?;
                    frames.push(frame, out.reborrow())?;
                }
                Some('|') => {
                    let frame = frames.current_mut(out.reborrow())?;
                    if frame.children.is_empty() {
                        return Err(out.fail(self.syntax(
                            RegexSyntaxErrorKind::EmptyAlternationBranch,
                            self.position,
                            self.position + 1,
                        )));
                    }
                    let branch = self.finish_children(&mut frame.children, out.reborrow())?;
                    frame.branches.try_push(branch, out.reborrow())?;
                    frame.after_bar = true;
                    self.advance_char(out.reborrow())?;
                }
                Some(')') | None => {
                    let current = self.current_char(out.reborrow())?;
                    if current == Some(')') && frames.len() == 1 {
                        return Err(out.fail(self.syntax(
                            RegexSyntaxErrorKind::UnexpectedToken,
                            self.position,
                            self.position + 1,
                        )));
                    }
                    let mut frame = frames.pop(out.reborrow())?;
                    if frame.after_bar {
                        return Err(out.fail(self.syntax(
                            RegexSyntaxErrorKind::EmptyAlternationBranch,
                            self.position,
                            self.position + current.map_or(0, char::len_utf8),
                        )));
                    }
                    if current.is_none() && !frames.is_empty() {
                        return Err(out.fail(self.syntax(
                            RegexSyntaxErrorKind::UnclosedGroup,
                            self.pattern.len(),
                            self.pattern.len(),
                        )));
                    }
                    let last = self.finish_children(&mut frame.children, out.reborrow())?;
                    let inner = if frame.branches.is_empty() {
                        last
                    } else {
                        frame.branches.try_push(last, out.reborrow())?;
                        let first_branch = frame
                            .branches
                            .first()
                            .copied()
                            .ok_or(RegexCompileError::InternalInvariantFailure(
                                RegexErrorContext::new(PARSER_ALTERNATION_BRANCH_MISSING),
                            ))
                            .store_err_into(out.reborrow())?;
                        let start = self.node(first_branch, out.reborrow())?.span.start();
                        self.push_node(
                            NodeKind::Alternation(frame.branches),
                            start,
                            self.position,
                            out.reborrow(),
                        )?
                    };
                    if frames.is_empty() {
                        break Ok(inner);
                    }
                    self.advance_char(out.reborrow())?;
                    let group = self.push_node(
                        NodeKind::Group(inner),
                        frame.start,
                        self.position,
                        out.reborrow(),
                    )?;
                    let repeated = self.finish_repeat(group, true, frame.start, out.reborrow())?;
                    let outer = frames.current_mut(out.reborrow())?;
                    outer.children.try_push(repeated, out.reborrow())?;
                    outer.after_bar = false;
                }
                Some(_) => {
                    let node = self.parse_repeat(0, out.reborrow())?;
                    let frame = frames.current_mut(out.reborrow())?;
                    frame.children.try_push(node, out.reborrow())?;
                    frame.after_bar = false;
                }
            }
        }
    }

    fn finish_children<'slot>(
        &mut self,
        children: &mut ChargedVec<NodeId>,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, NodeId> {
        self.execution.consume(1, out.reborrow())?;
        match children.len() {
            0 => self.push_node(
                NodeKind::Empty,
                self.position,
                self.position,
                out.reborrow(),
            ),
            1 => match children.pop() {
                Some(child) => Ok(child),
                None => Err(out.fail(RegexCompileError::InternalInvariantFailure(
                    RegexErrorContext::new(PARSER_SINGLE_CHILD_MISSING),
                ))),
            },
            _ => {
                let first_child = children
                    .first()
                    .copied()
                    .ok_or(RegexCompileError::InternalInvariantFailure(
                        RegexErrorContext::new(PARSER_FIRST_CHILD_MISSING),
                    ))
                    .store_err_into(out.reborrow())?;
                let start = self.node(first_child, out.reborrow())?.span.start();
                let empty = ChargedVec::new(Some(self.execution), out.reborrow())?;
                let owned = replace(children, empty);
                self.push_node(
                    NodeKind::Concatenation(owned),
                    start,
                    self.position,
                    out.reborrow(),
                )
            }
        }
    }

    fn parse_repeat<'slot>(
        &mut self,
        _depth: usize,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, NodeId> {
        let atom_start = self.position;
        let (node, quantifiable) = match self.current_char(out.reborrow())? {
            Some('^') => {
                self.advance_char(out.reborrow())?;
                (
                    self.push_node(NodeKind::Start, atom_start, self.position, out.reborrow())?,
                    false,
                )
            }
            Some('$') => {
                self.advance_char(out.reborrow())?;
                (
                    self.push_node(NodeKind::End, atom_start, self.position, out.reborrow())?,
                    false,
                )
            }
            Some('.') => {
                self.advance_char(out.reborrow())?;
                let class =
                    CharClass::dot_class(self.remaining_ranges(), self.execution, out.reborrow())?;
                self.record_ranges(class.range_count(), out.reborrow())?;
                (
                    self.push_node(
                        NodeKind::CharacterClass(class),
                        atom_start,
                        self.position,
                        out.reborrow(),
                    )?,
                    true,
                )
            }
            Some('[') => {
                let (class, span) =
                    character_class::CharacterClassParser::parse(self, out.reborrow())?;
                self.record_ranges(class.range_count(), out.reborrow())?;
                (
                    self.push_node(
                        NodeKind::CharacterClass(class),
                        span.start(),
                        span.end(),
                        out.reborrow(),
                    )?,
                    true,
                )
            }
            Some('\\') => {
                self.advance_char(out.reborrow())?;
                match self.parse_escape(atom_start, true, out.reborrow())? {
                    EscapeValue::Literal(ch) => (
                        self.push_node(
                            NodeKind::Literal(ch),
                            atom_start,
                            self.position,
                            out.reborrow(),
                        )?,
                        true,
                    ),
                    EscapeValue::Class(class) => {
                        self.record_ranges(class.range_count(), out.reborrow())?;
                        (
                            self.push_node(
                                NodeKind::CharacterClass(class),
                                atom_start,
                                self.position,
                                out.reborrow(),
                            )?,
                            true,
                        )
                    }
                }
            }
            Some(ch) if !character_class::CharacterClassParser::is_regex_metachar(ch) => {
                self.advance_char(out.reborrow())?;
                (
                    self.push_node(
                        NodeKind::Literal(ch),
                        atom_start,
                        self.position,
                        out.reborrow(),
                    )?,
                    true,
                )
            }
            Some(_) => {
                let end =
                    self.position + self.current_char(out.reborrow())?.map_or(0, char::len_utf8);
                return Err(out.fail(self.syntax(
                    RegexSyntaxErrorKind::UnexpectedToken,
                    self.position,
                    end,
                )));
            }
            None => {
                return Err(out.fail(self.syntax(
                    RegexSyntaxErrorKind::UnexpectedToken,
                    self.position,
                    self.position,
                )));
            }
        };

        self.finish_repeat(node, quantifiable, atom_start, out)
    }

    fn finish_repeat<'slot>(
        &mut self,
        node: NodeId,
        quantifiable: bool,
        atom_start: usize,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, NodeId> {
        if !quantifiable {
            return Ok(node);
        }
        let Some(kind) = self.parse_quantifier(out.reborrow())? else {
            return Ok(node);
        };
        let repeated = self.push_node(
            NodeKind::Repetition { node, kind },
            atom_start,
            self.position,
            out.reborrow(),
        )?;
        let current = self.current_char(out.reborrow())?;
        if character_class::CharacterClassParser::is_quantifier_start(current) {
            let end = self.position + current.map_or(0, char::len_utf8);
            return Err(out.fail(self.syntax(
                RegexSyntaxErrorKind::RepeatedQuantifier,
                self.position,
                end,
            )));
        }
        Ok(repeated)
    }

    fn parse_quantifier<'slot>(
        &mut self,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, Option<Repetition>> {
        let start = self.position;
        let kind = match self.current_char(out.reborrow())? {
            Some('*') => {
                self.advance_char(out.reborrow())?;
                Some(Repetition::ZeroOrMore)
            }
            Some('+') => {
                self.advance_char(out.reborrow())?;
                Some(Repetition::OneOrMore)
            }
            Some('?') => {
                self.advance_char(out.reborrow())?;
                Some(Repetition::ZeroOrOne)
            }
            Some('{') => Some(self.parse_bounded_repetition(start, out.reborrow())?),
            _ => None,
        };
        Ok(kind)
    }

    fn parse_bounded_repetition<'slot>(
        &mut self,
        start: usize,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, Repetition> {
        self.advance_char(out.reborrow())?;
        let parsed_min = self.parse_decimal_bound(out.reborrow())?;
        let (min, min_exceeded) = match parsed_min {
            Some(bound) => bound,
            None => {
                let ch = self.current_char(out.reborrow())?;
                return Err(out.fail(self.invalid_repetition(start, ch)));
            }
        };
        let max = match self.current_char(out.reborrow())? {
            Some('}') => {
                self.advance_char(out.reborrow())?;
                Some(min)
            }
            Some(',') => {
                self.advance_char(out.reborrow())?;
                if self.current_char(out.reborrow())? == Some('}') {
                    self.advance_char(out.reborrow())?;
                    None
                } else {
                    let parsed_max = self.parse_decimal_bound(out.reborrow())?;
                    let (max, max_exceeded) = match parsed_max {
                        Some(bound) => bound,
                        None => {
                            let ch = self.current_char(out.reborrow())?;
                            return Err(out.fail(self.invalid_repetition(start, ch)));
                        }
                    };
                    let closing = self.current_char(out.reborrow())?;
                    if closing != Some('}') {
                        return Err(out.fail(self.invalid_repetition(start, closing)));
                    }
                    self.advance_char(out.reborrow())?;
                    if min_exceeded || max_exceeded {
                        return Err(out.fail(self.repetition_bound_exceeded(start)));
                    }
                    if min > max {
                        return Err(out.fail(self.syntax(
                            RegexSyntaxErrorKind::InvalidRepetition,
                            start,
                            self.position,
                        )));
                    }
                    Some(max)
                }
            }
            None => {
                return Err(out.fail(self.syntax(
                    RegexSyntaxErrorKind::InvalidRepetition,
                    self.pattern.len(),
                    self.pattern.len(),
                )));
            }
            _ => {
                let ch = self.current_char(out.reborrow())?;
                return Err(out.fail(self.invalid_repetition(start, ch)));
            }
        };
        if min_exceeded {
            return Err(out.fail(self.repetition_bound_exceeded(start)));
        }
        Ok(Repetition::Bounded { min, max })
    }

    fn parse_decimal_bound<'slot>(
        &mut self,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, Option<(u16, bool)>> {
        let mut found = false;
        let mut exceeded = false;
        let mut value = 0u16;
        while let Some(ch) = self.current_char(out.reborrow())? {
            if !ch.is_ascii_digit() {
                break;
            }
            found = true;
            let digit = (ch as u8 - b'0') as u16;
            if value > 255 || value * 10 + digit > 255 {
                exceeded = true;
            } else {
                value = value * 10 + digit;
            }
            self.advance_char(out.reborrow())?;
        }
        Ok(found.then_some((value, exceeded)))
    }

    fn parse_escape<'slot>(
        &mut self,
        start: usize,
        enforce_range_limit: bool,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, EscapeValue> {
        let Some(ch) = self.current_char(out.reborrow())? else {
            return Err(out.fail(self.syntax(
                RegexSyntaxErrorKind::InvalidEscape,
                start,
                self.pattern.len(),
            )));
        };
        self.advance_char(out.reborrow())?;
        let literal = match ch {
            'n' => Some('\n'),
            'r' => Some('\r'),
            't' => Some('\t'),
            '-' => Some('-'),
            value if character_class::CharacterClassParser::is_regex_metachar(value) => Some(value),
            _ => None,
        };
        if let Some(literal) = literal {
            return Ok(EscapeValue::Literal(literal));
        }
        let available_ranges = if enforce_range_limit {
            self.remaining_ranges()
        } else {
            usize::MAX
        };
        let Some(class) =
            CharClass::shortcut(ch, available_ranges, self.execution, out.reborrow())?
        else {
            return Err(out.fail(self.syntax(
                RegexSyntaxErrorKind::InvalidEscape,
                start,
                self.position,
            )));
        };
        Ok(EscapeValue::Class(class))
    }

    fn push_node<'slot>(
        &mut self,
        kind: NodeKind,
        start: usize,
        end: usize,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, NodeId> {
        self.nodes.try_reserve(1, out.reborrow())?;
        let id = NodeId(self.nodes.len());
        self.nodes.try_push(
            Node {
                kind,
                span: PatternSpan::new(start, end),
            },
            out.reborrow(),
        )?;
        Ok(id)
    }

    fn node<'slot>(
        &self,
        id: NodeId,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, &Node> {
        self.nodes
            .get(id.0)
            .ok_or(RegexCompileError::InternalInvariantFailure(
                RegexErrorContext::new(PARSER_NODE_MISSING),
            ))
            .store_err_into(out.reborrow())
    }

    fn record_ranges<'slot>(
        &mut self,
        count: usize,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, ()> {
        let total = self
            .range_count
            .checked_add(count)
            .ok_or(RegexCompileError::ResourceLimitExceeded(
                RegexResource::CharacterRangeCount,
            ))
            .store_err_into(out.reborrow())?;
        if total > self.max_ranges {
            return Err(out.fail(RegexCompileError::ResourceLimitExceeded(
                RegexResource::CharacterRangeCount,
            )));
        }
        self.range_count = total;
        Ok(())
    }

    fn remaining_ranges(&self) -> usize {
        self.max_ranges.saturating_sub(self.range_count)
    }

    fn invalid_repetition(&self, start: usize, ch: Option<char>) -> RegexCompileError {
        let position = if ch.is_none() {
            self.pattern.len()
        } else {
            start
        };
        self.syntax(
            RegexSyntaxErrorKind::InvalidRepetition,
            position,
            self.position + ch.map_or(0, char::len_utf8),
        )
    }

    fn repetition_bound_exceeded(&self, start: usize) -> RegexCompileError {
        self.syntax(
            RegexSyntaxErrorKind::RepetitionBoundExceeded,
            start,
            self.position,
        )
    }

    fn syntax(&self, kind: RegexSyntaxErrorKind, start: usize, end: usize) -> RegexCompileError {
        RegexCompileError::Syntax(RegexSyntaxError::new(kind, PatternSpan::new(start, end)))
    }

    pub(super) fn pattern(&self) -> &str {
        self.pattern
    }

    pub(super) fn position(&self) -> usize {
        self.position
    }

    pub(super) fn current_char<'slot, 'borrow>(
        &self,
        out: Out<'slot, 'borrow, RegexCompileError>,
    ) -> OResult<'slot, Option<char>> {
        self.read_at(self.position, out)
    }

    fn read_at<'slot>(
        &self,
        position: usize,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, Option<char>> {
        self.execution.consume(1, out.reborrow())?;
        let Some(first) = self.pattern.as_bytes().get(position).copied() else {
            return Ok(None);
        };
        let length = if first < 0x80 {
            1
        } else if first < 0xE0 {
            2
        } else if first < 0xF0 {
            3
        } else {
            4
        };
        self.execution.consume(length, out.reborrow())?;
        Ok(self.pattern[position..].chars().next())
    }

    pub(super) fn peek_after_current<'slot>(
        &self,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, Option<char>> {
        let Some(ch) = self.current_char(out.reborrow())? else {
            return Ok(None);
        };
        self.read_at(self.position + ch.len_utf8(), out.reborrow())
    }

    pub(super) fn advance_char<'slot>(
        &mut self,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, Option<char>> {
        let Some(ch) = self.current_char(out.reborrow())? else {
            return Ok(None);
        };
        self.position += ch.len_utf8();
        Ok(Some(ch))
    }

    pub(super) fn range_temporary_limit(&self) -> usize {
        self.pattern.len().saturating_mul(3).max(6)
    }

    pub(super) fn syntax_error(
        &self,
        kind: RegexSyntaxErrorKind,
        start: usize,
        end: usize,
    ) -> RegexCompileError {
        self.syntax(kind, start, end)
    }

    pub(super) fn new_class_builder<'slot, 'borrow>(
        &self,
        out: Out<'slot, 'borrow, RegexCompileError>,
    ) -> OResult<'slot, CharClassBuilder> {
        CharClassBuilder::new(self.range_temporary_limit(), self.execution, out)
    }

    pub(super) fn finish_class<'slot, 'borrow>(
        &self,
        builder: CharClassBuilder,
        negated: bool,
        out: Out<'slot, 'borrow, RegexCompileError>,
    ) -> OResult<'slot, CharClass> {
        builder.finish(negated, self.remaining_ranges(), out)
    }

    pub(super) fn parse_escape_for_class<'slot, 'borrow>(
        &mut self,
        slash_start: usize,
        out: Out<'slot, 'borrow, RegexCompileError>,
    ) -> OResult<'slot, EscapeValue> {
        self.parse_escape(slash_start, false, out)
    }
}

impl EscapeValue {
    pub(super) fn literal(&self) -> Option<char> {
        match self {
            EscapeValue::Literal(ch) => Some(*ch),
            EscapeValue::Class(_) => None,
        }
    }

    pub(super) fn class(&self) -> Option<&CharClass> {
        match self {
            EscapeValue::Literal(_) => None,
            EscapeValue::Class(class) => Some(class),
        }
    }
}

impl GroupFrames {
    fn new<'slot>(
        execution: &ExecutionBudget,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, Self> {
        Ok(Self {
            values: ChargedVec::new(Some(execution), out.reborrow())?,
        })
    }

    fn push<'slot>(
        &mut self,
        frame: GroupFrame,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, ()> {
        self.values.try_push(frame, out.reborrow())?;
        Ok(())
    }

    fn pop<'slot>(
        &mut self,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, GroupFrame> {
        self.values
            .pop()
            .ok_or(RegexCompileError::InternalInvariantFailure(
                RegexErrorContext::new(GROUP_FRAME_POP_MISSING),
            ))
            .store_err_into(out.reborrow())
    }

    fn current_mut<'slot>(
        &mut self,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, &mut GroupFrame> {
        self.values
            .last_mut()
            .ok_or(RegexCompileError::InternalInvariantFailure(
                RegexErrorContext::new(GROUP_FRAME_CURRENT_MISSING),
            ))
            .store_err_into(out.reborrow())
    }

    fn len(&self) -> usize {
        self.values.len()
    }

    fn is_empty(&self) -> bool {
        self.values.is_empty()
    }
}

impl GroupFrame {
    fn new<'slot>(
        start: usize,
        execution: &ExecutionBudget,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, Self> {
        Ok(Self {
            start,
            children: ChargedVec::new(Some(execution), out.reborrow())?,
            branches: ChargedVec::new(Some(execution), out.reborrow())?,
            after_bar: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use alloc::{format, string::String, vec::Vec};
    use core::fmt::{Debug, Display, Formatter, Result as FmtResult};
    use opeo::{ErrSlot, OResult, Out};

    use typol_core::{BudgetError, ExecutionBudget, ExecutionLimits};
    use typol_policy::ResourceLimits;

    use crate::{
        RegexCompileError, RegexResource, RegexSyntaxErrorKind,
        char_class::{CharClass, ScalarRange},
        parser::{Ast, Node, NodeId, NodeKind, Repetition},
    };

    use super::Parser;

    #[derive(Debug)]
    struct TestFailure(String);

    impl Display for TestFailure {
        fn fmt(&self, formatter: &mut Formatter<'_>) -> FmtResult {
            formatter.write_str(&self.0)
        }
    }

    impl From<RegexCompileError> for TestFailure {
        fn from(error: RegexCompileError) -> Self {
            Self(format!("{error:?}"))
        }
    }

    fn check(condition: bool, message: &str) -> Result<(), TestFailure> {
        if condition {
            Ok(())
        } else {
            Err(TestFailure(message.into()))
        }
    }

    fn check_eq<T>(actual: T, expected: T, message: &str) -> Result<(), TestFailure>
    where
        T: Debug + PartialEq,
    {
        if actual == expected {
            Ok(())
        } else {
            Err(TestFailure(format!(
                "{message}: actual value {actual:?}, expected {expected:?}"
            )))
        }
    }

    fn require<T>(value: Option<T>, message: &str) -> Result<T, TestFailure> {
        value.ok_or_else(|| TestFailure(message.into()))
    }

    #[derive(Debug)]
    enum Attempt<T, E> {
        Success(T),
        Failure(E),
    }

    struct ParseErrors {
        budget: ErrSlot<BudgetError>,
        parse: ErrSlot<RegexCompileError>,
    }

    impl ParseErrors {
        fn new() -> Self {
            Self {
                budget: ErrSlot::new(),
                parse: ErrSlot::new(),
            }
        }
    }

    fn attempt<T, E>(
        errors: &mut ErrSlot<E>,
        operation: impl for<'slot> FnOnce(Out<'slot, 'slot, E>) -> OResult<'slot, T>,
    ) -> Attempt<T, E> {
        match errors.try_call(operation) {
            Ok(value) => Attempt::Success(value),
            Err(error) => Attempt::Failure(error.take()),
        }
    }

    fn parse(
        pattern: &str,
        limits: &ResourceLimits,
        errors: &mut ParseErrors,
    ) -> Attempt<Ast, RegexCompileError> {
        let execution = match attempt(&mut errors.budget, |out| {
            ExecutionBudget::new(ExecutionLimits::new(67_108_864, 67_108_864), out)
        }) {
            Attempt::Success(execution) => execution,
            Attempt::Failure(error) => {
                return Attempt::Failure(RegexCompileError::from(error));
            }
        };
        attempt(&mut errors.parse, |out| {
            Parser::parse_with_range_limit(
                pattern,
                limits,
                &execution,
                limits.max_regex_ranges_per_pattern(),
                out,
            )
        })
    }

    fn parse_success(
        pattern: &str,
        limits: &ResourceLimits,
        errors: &mut ParseErrors,
    ) -> Result<Ast, TestFailure> {
        match parse(pattern, limits, errors) {
            Attempt::Success(ast) => Ok(ast),
            Attempt::Failure(error) => Err(TestFailure(format!("{error:?}"))),
        }
    }

    fn node<'ast>(
        ast: &'ast Ast,
        id: NodeId,
        errors: &mut ParseErrors,
    ) -> Result<&'ast Node, TestFailure> {
        match attempt(&mut errors.parse, |out| ast.node(id, out)) {
            Attempt::Success(node) => Ok(node),
            Attempt::Failure(error) => Err(TestFailure(format!("{error:?}"))),
        }
    }

    fn assert_resource(
        result: Attempt<Ast, RegexCompileError>,
        expected: RegexResource,
    ) -> Result<(), TestFailure> {
        match result {
            Attempt::Failure(RegexCompileError::ResourceLimitExceeded(actual)) => {
                check_eq(actual, expected, "resource limit kind does not match")
            }
            other => Err(TestFailure(format!(
                "expected a resource error, got {other:?}"
            ))),
        }
    }

    fn assert_syntax(
        errors: &mut ParseErrors,
        pattern: &str,
        kind: RegexSyntaxErrorKind,
        span: (usize, usize),
    ) -> Result<(), TestFailure> {
        match parse(pattern, &ResourceLimits::default(), errors) {
            Attempt::Failure(RegexCompileError::Syntax(error)) => {
                check_eq(error.kind(), kind, "syntax error kind does not match")?;
                check_eq(
                    (error.span().start(), error.span().end()),
                    span,
                    "syntax error span does not match",
                )
            }
            other => Err(TestFailure(format!(
                "expected a syntax error, got {other:?}"
            ))),
        }
    }

    fn all_classes(ast: &Ast) -> impl Iterator<Item = &CharClass> {
        ast.nodes.iter().filter_map(|node| match &node.kind {
            NodeKind::CharacterClass(class) => Some(class),
            _ => None,
        })
    }

    #[test]
    fn parses_empty_expressions_groups_and_alternation() -> Result<(), TestFailure> {
        let mut errors = ParseErrors::new();
        let empty = parse_success("", &ResourceLimits::default(), &mut errors)?;
        check(
            matches!(
                &node(&empty, empty.root(), &mut errors)?.kind,
                NodeKind::Empty
            ),
            "empty pattern should produce an empty node",
        )?;

        let grouped = parse_success("()", &ResourceLimits::default(), &mut errors)?;
        check(
            matches!(
                &node(&grouped, grouped.root(), &mut errors)?.kind,
                NodeKind::Group(_)
            ),
            "empty group should produce a group node",
        )?;

        let alternatives = parse_success("a|bc", &ResourceLimits::default(), &mut errors)?;
        let NodeKind::Alternation(branches) =
            &node(&alternatives, alternatives.root(), &mut errors)?.kind
        else {
            return Err(TestFailure("expected an alternation node".into()));
        };
        check_eq(branches.len(), 2, "alternation branch count does not match")?;
        let first_id = require(
            branches.first().copied(),
            "alternation is missing its first branch",
        )?;
        let second_id = require(
            branches.get(1).copied(),
            "alternation is missing its second branch",
        )?;
        check(
            matches!(
                &node(&alternatives, first_id, &mut errors)?.kind,
                NodeKind::Literal('a')
            ),
            "first alternation branch should be a literal",
        )?;
        check(
            matches!(
                &node(&alternatives, second_id, &mut errors)?.kind,
                NodeKind::Concatenation(_)
            ),
            "second alternation branch should be a concatenation node",
        )
    }

    #[test]
    fn accepts_each_supported_atom_and_quantifier_form() -> Result<(), TestFailure> {
        let mut errors = ParseErrors::new();
        let patterns = [
            "",
            "()",
            "a",
            ".",
            "[a]",
            r"\n",
            r"\d",
            "^",
            "$",
            "ab",
            "a|b",
            "(a)",
            "a*",
            "a+",
            "a?",
            "a{1}",
            "a{1,}",
            "a{1,2}",
            "[-a]",
            "[a-]",
            "[a-z]",
            r"[a\-z]",
            "[^a]",
            r"\.\^\$\|\(\)\[\]\{\}\*\+\?\\\-",
        ];
        for pattern in patterns {
            check(
                matches!(
                    parse(pattern, &ResourceLimits::default(), &mut errors),
                    Attempt::Success(_)
                ),
                &format!("pattern should be valid: {pattern:?}"),
            )?;
        }
        Ok(())
    }

    #[test]
    fn parses_all_quantifiers_and_preserves_spans() -> Result<(), TestFailure> {
        let mut errors = ParseErrors::new();
        let ast = parse_success(
            "^a*b+c?d{2}e{2,}f{1,3}$",
            &ResourceLimits::default(),
            &mut errors,
        )?;
        let repetitions: Vec<_> = ast
            .nodes
            .iter()
            .filter_map(|node| match &node.kind {
                NodeKind::Repetition { kind, .. } => Some((*kind, node.span)),
                _ => None,
            })
            .collect();
        let actual_kinds: Vec<_> = repetitions.iter().map(|(kind, _)| *kind).collect();
        let expected_kinds = Vec::from([
            Repetition::ZeroOrMore,
            Repetition::OneOrMore,
            Repetition::ZeroOrOne,
            Repetition::Bounded {
                min: 2,
                max: Some(2),
            },
            Repetition::Bounded { min: 2, max: None },
            Repetition::Bounded {
                min: 1,
                max: Some(3),
            },
        ]);
        check_eq(
            actual_kinds,
            expected_kinds,
            "quantifier parse result does not match",
        )?;
        let (_, last_span) = require(repetitions.last(), "pattern should contain a quantifier")?;
        check_eq(
            (last_span.start(), last_span.end()),
            (16, 22),
            "trailing quantifier span does not match",
        )?;
        check(
            ast.nodes
                .iter()
                .any(|node| matches!(&node.kind, NodeKind::Start)),
            "expected a start anchor",
        )?;
        check(
            ast.nodes
                .iter()
                .any(|node| matches!(&node.kind, NodeKind::End)),
            "expected an end anchor",
        )
    }

    #[test]
    fn parses_escapes_and_character_classes_as_scalar_ranges() -> Result<(), TestFailure> {
        let mut errors = ParseErrors::new();
        let ast = parse_success(
            r"\n\r\t\.\-\d\D\w\W\s\S.[^a-cb-d]",
            &ResourceLimits::default(),
            &mut errors,
        )?;
        let classes: Vec<_> = all_classes(&ast).collect();
        check_eq(classes.len(), 8, "character class count does not match")?;
        let mut classes = classes.into_iter();
        let digit = require(classes.next(), "expected a digit character class")?;
        check_eq(
            digit.ranges(),
            &[ScalarRange { start: 48, end: 57 }][..],
            "digit character class ranges do not match",
        )?;
        let expected_counts = [3, 4, 6, 2, 4, 3, 3];
        for (class, expected) in classes.by_ref().zip(expected_counts) {
            check_eq(
                class.range_count(),
                expected,
                "character class range count does not match",
            )?;
        }
        let negated_digit = require(all_classes(&ast).nth(6), "expected a dot character class")?;
        check_eq(
            negated_digit.ranges(),
            &[
                ScalarRange { start: 0, end: 9 },
                ScalarRange {
                    start: 11,
                    end: 0xD7FF,
                },
                ScalarRange {
                    start: 0xE000,
                    end: 0x10FFFF,
                },
            ][..],
            "dot character class ranges do not match",
        )?;
        let negated_class = require(
            all_classes(&ast).nth(7),
            "expected a negated character class",
        )?;
        check_eq(
            negated_class.ranges(),
            &[
                ScalarRange {
                    start: 0,
                    end: 0x60,
                },
                ScalarRange {
                    start: 0x65,
                    end: 0xD7FF,
                },
                ScalarRange {
                    start: 0xE000,
                    end: 0x10FFFF,
                },
            ][..],
            "negated character class ranges do not match",
        )
    }

    #[test]
    fn range_crossing_surrogate_gap_stores_only_unicode_scalars() -> Result<(), TestFailure> {
        let mut errors = ParseErrors::new();
        let mut pattern = String::from("[");
        pattern.push('\u{D7FF}');
        pattern.push('-');
        pattern.push('\u{E000}');
        pattern.push(']');
        let ast = parse_success(&pattern, &ResourceLimits::default(), &mut errors)?;
        let class = require(all_classes(&ast).next(), "expected a character class")?;
        check_eq(
            class.ranges(),
            &[
                ScalarRange {
                    start: 0xD7FF,
                    end: 0xD7FF,
                },
                ScalarRange {
                    start: 0xE000,
                    end: 0xE000,
                },
            ][..],
            "a range crossing the surrogate gap should keep only Unicode scalars",
        )
    }

    #[test]
    fn rejects_each_invalid_grammar_family_with_stable_span() -> Result<(), TestFailure> {
        let mut errors = ParseErrors::new();
        assert_syntax(
            &mut errors,
            "|",
            RegexSyntaxErrorKind::EmptyAlternationBranch,
            (0, 1),
        )?;
        assert_syntax(
            &mut errors,
            "a|",
            RegexSyntaxErrorKind::EmptyAlternationBranch,
            (2, 2),
        )?;
        assert_syntax(
            &mut errors,
            "a||b",
            RegexSyntaxErrorKind::EmptyAlternationBranch,
            (2, 3),
        )?;
        assert_syntax(
            &mut errors,
            r"\q",
            RegexSyntaxErrorKind::InvalidEscape,
            (0, 2),
        )?;
        assert_syntax(
            &mut errors,
            r"\",
            RegexSyntaxErrorKind::InvalidEscape,
            (0, 1),
        )?;
        assert_syntax(
            &mut errors,
            "[]",
            RegexSyntaxErrorKind::EmptyCharacterClass,
            (0, 2),
        )?;
        assert_syntax(
            &mut errors,
            "[^]",
            RegexSyntaxErrorKind::EmptyCharacterClass,
            (0, 3),
        )?;
        assert_syntax(
            &mut errors,
            "[z-a]",
            RegexSyntaxErrorKind::InvalidCharacterRange,
            (1, 4),
        )?;
        assert_syntax(
            &mut errors,
            "[a-\\d]",
            RegexSyntaxErrorKind::InvalidCharacterRange,
            (1, 3),
        )?;
        assert_syntax(
            &mut errors,
            "[a-z",
            RegexSyntaxErrorKind::UnclosedCharacterClass,
            (4, 4),
        )?;
        assert_syntax(
            &mut errors,
            "[a-",
            RegexSyntaxErrorKind::UnclosedCharacterClass,
            (3, 3),
        )?;
        assert_syntax(
            &mut errors,
            "(",
            RegexSyntaxErrorKind::UnclosedGroup,
            (1, 1),
        )?;
        assert_syntax(
            &mut errors,
            "a**",
            RegexSyntaxErrorKind::RepeatedQuantifier,
            (2, 3),
        )?;
        assert_syntax(
            &mut errors,
            "a*?",
            RegexSyntaxErrorKind::RepeatedQuantifier,
            (2, 3),
        )?;
        assert_syntax(
            &mut errors,
            "a{1}{2}",
            RegexSyntaxErrorKind::RepeatedQuantifier,
            (4, 5),
        )?;
        assert_syntax(
            &mut errors,
            "a{256}",
            RegexSyntaxErrorKind::RepetitionBoundExceeded,
            (1, 6),
        )?;
        assert_syntax(
            &mut errors,
            "a{3,2}",
            RegexSyntaxErrorKind::InvalidRepetition,
            (1, 6),
        )?;
        assert_syntax(
            &mut errors,
            "a{",
            RegexSyntaxErrorKind::InvalidRepetition,
            (2, 2),
        )?;
        assert_syntax(
            &mut errors,
            "a{,1}",
            RegexSyntaxErrorKind::InvalidRepetition,
            (1, 3),
        )?;
        assert_syntax(
            &mut errors,
            "a{1,2,3}",
            RegexSyntaxErrorKind::InvalidRepetition,
            (1, 6),
        )?;
        assert_syntax(
            &mut errors,
            r"\p{L}",
            RegexSyntaxErrorKind::InvalidEscape,
            (0, 2),
        )?;
        assert_syntax(
            &mut errors,
            "*a",
            RegexSyntaxErrorKind::UnexpectedToken,
            (0, 1),
        )?;
        assert_syntax(
            &mut errors,
            ")",
            RegexSyntaxErrorKind::UnexpectedToken,
            (0, 1),
        )
    }

    #[test]
    fn spans_use_utf8_byte_offsets() -> Result<(), TestFailure> {
        let mut errors = ParseErrors::new();
        assert_syntax(
            &mut errors,
            "[é-á]",
            RegexSyntaxErrorKind::InvalidCharacterRange,
            (1, 6),
        )
    }

    #[test]
    fn checks_pattern_nesting_and_character_range_limits() -> Result<(), TestFailure> {
        let mut errors = ParseErrors::new();
        let mut limits = ResourceLimits::default();
        limits.set_max_regex_pattern_bytes(1);
        assert_resource(
            parse("ab", &limits, &mut errors),
            RegexResource::PatternBytes,
        )?;

        let mut at_limit = ResourceLimits::default();
        at_limit.set_max_regex_nesting_depth(64);
        let nested = format!("{}a{}", "(".repeat(64), ")".repeat(64));
        check(
            matches!(parse(&nested, &at_limit, &mut errors), Attempt::Success(_)),
            "boundary nesting depth should be accepted",
        )?;
        let too_deep = format!("{}a{}", "(".repeat(65), ")".repeat(65));
        assert_resource(
            parse(&too_deep, &ResourceLimits::default(), &mut errors),
            RegexResource::NestingDepth,
        )?;

        let mut no_ranges = ResourceLimits::default();
        no_ranges.set_max_regex_ranges_per_pattern(0);
        assert_resource(
            parse("[a]", &no_ranges, &mut errors),
            RegexResource::CharacterRangeCount,
        )?;

        let mut one_range = ResourceLimits::default();
        one_range.set_max_regex_ranges_per_pattern(1);
        check(
            matches!(
                parse("[a-ce-gd]", &one_range, &mut errors),
                Attempt::Success(_)
            ),
            "a single merged range should be accepted",
        )?;

        let mut two_ranges = ResourceLimits::default();
        two_ranges.set_max_regex_ranges_per_pattern(2);
        check(
            matches!(
                parse(r"[\W\w]", &two_ranges, &mut errors),
                Attempt::Success(_)
            ),
            "two character ranges should be accepted",
        )?;
        assert_resource(
            parse(r"[a][b]", &one_range, &mut errors),
            RegexResource::CharacterRangeCount,
        )
    }

    #[test]
    fn syntax_error_helper_distinguishes_resource_errors() -> Result<(), TestFailure> {
        let mut errors = ParseErrors::new();
        assert_syntax(
            &mut errors,
            "a|",
            RegexSyntaxErrorKind::EmptyAlternationBranch,
            (2, 2),
        )
    }
}
