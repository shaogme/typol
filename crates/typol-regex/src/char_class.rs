use crate::{
    RegexCompileError, RegexErrorContext, RegexMatchError, RegexResource,
    error::{
        CHARACTER_CLASS_BUDGET_MISSING, CHARACTER_CLASS_PREVIOUS_RANGE_MISSING,
        CHARACTER_CLASS_RANGE_MISSING, CHARACTER_CLASS_SLOT_MISSING,
        MATCH_CHARACTER_CLASS_RANGE_MISSING,
    },
    matcher::RegexMatcher,
};
use opeo::{OResult, Out, ResultOutExt};
use typol_core::{ChargedVec, RegexExecutionBudget};

const SURROGATE_START: u32 = 0xD800;
const SURROGATE_END: u32 = 0xDFFF;
const SCALAR_MAX: u32 = 0x10FFFF;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ScalarRange {
    pub(crate) start: u32,
    pub(crate) end: u32,
}

pub(crate) struct CharClass {
    ranges: ChargedVec<ScalarRange>,
}

impl CharClass {
    pub(crate) fn singleton<'slot>(
        scalar: char,
        budget: &RegexExecutionBudget,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, Self> {
        let mut ranges = ChargedVec::new_regex(Some(budget), out.reborrow())?;
        ranges.try_reserve_exact(1, out.reborrow())?;
        ranges.try_push(
            ScalarRange {
                start: scalar as u32,
                end: scalar as u32,
            },
            out.reborrow(),
        )?;
        Ok(Self { ranges })
    }

    pub(crate) fn try_clone<'slot>(
        &self,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, Self> {
        let budget = self
            .ranges
            .regex_budget(out.reborrow())?
            .ok_or(RegexCompileError::InternalInvariantFailure(
                RegexErrorContext::new(CHARACTER_CLASS_BUDGET_MISSING),
            ))
            .store_err_into(out.reborrow())?;
        let mut ranges = ChargedVec::new_regex(Some(&budget), out.reborrow())?;
        ranges.try_reserve_exact(self.ranges.len(), out.reborrow())?;
        ranges.extend_from_slice(&self.ranges, out.reborrow())?;
        Ok(Self { ranges })
    }

    pub(crate) fn ranges(&self) -> &[ScalarRange] {
        &self.ranges
    }

    pub(crate) fn range_count(&self) -> usize {
        self.ranges.len()
    }

    pub(crate) fn matches<'slot>(
        &self,
        scalar: char,
        budget: &RegexExecutionBudget,
        mut out: Out<'slot, '_, RegexMatchError>,
    ) -> OResult<'slot, bool> {
        let value = scalar as u32;
        let mut low = 0;
        let mut high = self.ranges.len();
        while low < high {
            budget.consume_with(1, out.reborrow(), RegexMatcher::match_budget_error)?;
            let middle = low + (high - low) / 2;
            let range = self
                .ranges
                .get(middle)
                .copied()
                .ok_or(RegexMatchError::InternalInvariantFailure(
                    RegexErrorContext::new(MATCH_CHARACTER_CLASS_RANGE_MISSING),
                ))
                .store_err_into(out.reborrow())?;
            if value < range.start {
                high = middle;
            } else if value > range.end {
                low = middle + 1;
            } else {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

pub(crate) struct CharClassBuilder {
    ranges: ChargedVec<ScalarRange>,
    temporary_limit: usize,
}

impl CharClassBuilder {
    pub(crate) fn new<'slot>(
        temporary_limit: usize,
        budget: &RegexExecutionBudget,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, Self> {
        Ok(Self {
            ranges: ChargedVec::new_regex(Some(budget), out.reborrow())?,
            temporary_limit,
        })
    }

    pub(crate) fn add_scalar<'slot, 'borrow>(
        &mut self,
        scalar: char,
        out: Out<'slot, 'borrow, RegexCompileError>,
    ) -> OResult<'slot, ()> {
        self.add_range(scalar as u32, scalar as u32, out)
    }

    pub(crate) fn add_range<'slot>(
        &mut self,
        start: u32,
        end: u32,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, ()> {
        if start <= SURROGATE_END && end >= SURROGATE_START {
            if start < SURROGATE_START {
                self.push_range(start, SURROGATE_START - 1, out.reborrow())?;
            }
            if end > SURROGATE_END {
                self.push_range(SURROGATE_END + 1, end, out.reborrow())?;
            }
            return Ok(());
        }
        self.push_range(start, end, out)
    }

    pub(crate) fn add_class<'slot>(
        &mut self,
        class: &CharClass,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, ()> {
        let budget = self
            .ranges
            .regex_budget(out.reborrow())?
            .ok_or(RegexCompileError::InternalInvariantFailure(
                RegexErrorContext::new(CHARACTER_CLASS_BUDGET_MISSING),
            ))
            .store_err_into(out.reborrow())?
            .try_clone(out.reborrow())?;
        for range in class.ranges() {
            budget.consume(1, out.reborrow())?;
            self.push_range(range.start, range.end, out.reborrow())?;
        }
        Ok(())
    }

    pub(crate) fn finish<'slot>(
        mut self,
        negated: bool,
        available_ranges: usize,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, CharClass> {
        normalize(&mut self.ranges, out.reborrow())?;
        if negated {
            let budget = self
                .ranges
                .regex_budget(out.reborrow())?
                .ok_or(RegexCompileError::InternalInvariantFailure(
                    RegexErrorContext::new(CHARACTER_CLASS_BUDGET_MISSING),
                ))
                .store_err_into(out.reborrow())?;
            self.ranges = complement(&self.ranges, available_ranges, &budget, out.reborrow())?;
        } else if self.ranges.len() > available_ranges {
            return Err(out.fail(RegexCompileError::ResourceLimitExceeded(
                RegexResource::CharacterRangeCount,
            )));
        }
        Ok(CharClass {
            ranges: self.ranges,
        })
    }

    fn push_range<'slot>(
        &mut self,
        start: u32,
        end: u32,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, ()> {
        if self.ranges.len() >= self.temporary_limit {
            return Err(out.fail(RegexCompileError::ResourceLimitExceeded(
                RegexResource::CharacterRangeCount,
            )));
        }
        self.ranges.try_reserve(1, out.reborrow())?;
        self.ranges
            .try_push(ScalarRange { start, end }, out.reborrow())?;
        Ok(())
    }
}

impl CharClass {
    pub(crate) fn shortcut<'slot>(
        escape: char,
        available_ranges: usize,
        budget: &RegexExecutionBudget,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, Option<CharClass>> {
        let range_count = match escape {
            'd' => 1,
            'D' => 3,
            'w' => 4,
            'W' => 6,
            's' => 2,
            'S' => 4,
            _ => return Ok(None),
        };
        if range_count > available_ranges {
            return Err(out.fail(RegexCompileError::ResourceLimitExceeded(
                RegexResource::CharacterRangeCount,
            )));
        }
        let mut builder = CharClassBuilder::new(4, budget, out.reborrow())?;
        let negated = match escape {
            'd' | 'D' => {
                builder.add_range('0' as u32, '9' as u32, out.reborrow())?;
                escape == 'D'
            }
            'w' | 'W' => {
                builder.add_range('0' as u32, '9' as u32, out.reborrow())?;
                builder.add_range('A' as u32, 'Z' as u32, out.reborrow())?;
                builder.add_scalar('_', out.reborrow())?;
                builder.add_range('a' as u32, 'z' as u32, out.reborrow())?;
                escape == 'W'
            }
            's' | 'S' => {
                builder.add_range(0x09, 0x0D, out.reborrow())?;
                builder.add_scalar(' ', out.reborrow())?;
                escape == 'S'
            }
            _ => return Ok(None),
        };
        let class = builder.finish(negated, 6, out.reborrow())?;
        Ok(Some(class))
    }

    pub(crate) fn dot_class<'slot>(
        available_ranges: usize,
        budget: &RegexExecutionBudget,
        mut out: Out<'slot, '_, RegexCompileError>,
    ) -> OResult<'slot, CharClass> {
        if available_ranges < 2 {
            return Err(out.fail(RegexCompileError::ResourceLimitExceeded(
                RegexResource::CharacterRangeCount,
            )));
        }
        let mut builder = CharClassBuilder::new(3, budget, out.reborrow())?;
        builder.add_range(0, 0x09, out.reborrow())?;
        builder.add_range(0x0B, SCALAR_MAX, out.reborrow())?;
        builder.finish(false, available_ranges, out.reborrow())
    }
}

fn normalize<'slot>(
    ranges: &mut ChargedVec<ScalarRange>,
    mut out: Out<'slot, '_, RegexCompileError>,
) -> OResult<'slot, ()> {
    let budget = ranges
        .regex_budget(out.reborrow())?
        .ok_or(RegexCompileError::InternalInvariantFailure(
            RegexErrorContext::new(CHARACTER_CLASS_BUDGET_MISSING),
        ))
        .store_err_into(out.reborrow())?
        .try_clone(out.reborrow())?;
    ranges.sort_by(
        |left, right, mut compare_out| {
            budget.consume(1, compare_out.reborrow())?;
            Ok((left.start, left.end).cmp(&(right.start, right.end)))
        },
        out.reborrow(),
    )?;
    let mut merged = 0usize;
    for read in 0..ranges.len() {
        budget.consume(2, out.reborrow())?;
        let range = ranges
            .get(read)
            .copied()
            .ok_or(RegexCompileError::InternalInvariantFailure(
                RegexErrorContext::new(CHARACTER_CLASS_RANGE_MISSING),
            ))
            .store_err_into(out.reborrow())?;
        if merged == 0 {
            *ranges
                .get_mut(0)
                .ok_or(RegexCompileError::InternalInvariantFailure(
                    RegexErrorContext::new(CHARACTER_CLASS_SLOT_MISSING),
                ))
                .store_err_into(out.reborrow())? = range;
            merged = 1;
            continue;
        }
        let previous_index = merged
            .checked_sub(1)
            .ok_or(RegexCompileError::InternalInvariantFailure(
                RegexErrorContext::new(CHARACTER_CLASS_PREVIOUS_RANGE_MISSING),
            ))
            .store_err_into(out.reborrow())?;
        let previous = ranges
            .get_mut(previous_index)
            .ok_or(RegexCompileError::InternalInvariantFailure(
                RegexErrorContext::new(CHARACTER_CLASS_SLOT_MISSING),
            ))
            .store_err_into(out.reborrow())?;
        if range.start <= previous.end.saturating_add(1) {
            previous.end = previous.end.max(range.end);
        } else {
            *ranges
                .get_mut(merged)
                .ok_or(RegexCompileError::InternalInvariantFailure(
                    RegexErrorContext::new(CHARACTER_CLASS_SLOT_MISSING),
                ))
                .store_err_into(out.reborrow())? = range;
            merged += 1;
        }
    }
    ranges.truncate(merged);
    Ok(())
}

fn complement<'slot>(
    ranges: &[ScalarRange],
    available_ranges: usize,
    budget: &RegexExecutionBudget,
    mut out: Out<'slot, '_, RegexCompileError>,
) -> OResult<'slot, ChargedVec<ScalarRange>> {
    let mut count =
        complement_segment_count(ranges, 0, SURROGATE_START - 1, budget, out.reborrow())?;
    let second_count = complement_segment_count(
        ranges,
        SURROGATE_END + 1,
        SCALAR_MAX,
        budget,
        out.reborrow(),
    )?;
    count = count
        .checked_add(second_count)
        .ok_or(RegexCompileError::ResourceLimitExceeded(
            RegexResource::CharacterRangeCount,
        ))
        .store_err_into(out.reborrow())?;
    if count > available_ranges {
        return Err(out.fail(RegexCompileError::ResourceLimitExceeded(
            RegexResource::CharacterRangeCount,
        )));
    }

    let mut result = ChargedVec::new_regex(Some(budget), out.reborrow())?;
    result.try_reserve_exact(count, out.reborrow())?;
    append_complement_segment(
        &mut result,
        ranges,
        ScalarRange {
            start: 0,
            end: SURROGATE_START - 1,
        },
        budget,
        out.reborrow(),
    )?;
    append_complement_segment(
        &mut result,
        ranges,
        ScalarRange {
            start: SURROGATE_END + 1,
            end: SCALAR_MAX,
        },
        budget,
        out.reborrow(),
    )?;
    Ok(result)
}

fn complement_segment_count<'slot>(
    ranges: &[ScalarRange],
    segment_start: u32,
    segment_end: u32,
    budget: &RegexExecutionBudget,
    mut out: Out<'slot, '_, RegexCompileError>,
) -> OResult<'slot, usize> {
    let mut cursor = segment_start;
    let mut count = 0usize;
    for range in ranges {
        budget.consume(1, out.reborrow())?;
        if range.end < segment_start || range.start > segment_end {
            continue;
        }
        let start = range.start.max(segment_start);
        let end = range.end.min(segment_end);
        if cursor < start {
            count = count
                .checked_add(1)
                .ok_or(RegexCompileError::ResourceLimitExceeded(
                    RegexResource::CharacterRangeCount,
                ))
                .store_err_into(out.reborrow())?;
        }
        if end >= cursor {
            cursor = end.saturating_add(1);
        }
    }
    if cursor <= segment_end {
        count = count
            .checked_add(1)
            .ok_or(RegexCompileError::ResourceLimitExceeded(
                RegexResource::CharacterRangeCount,
            ))
            .store_err_into(out.reborrow())?;
    }
    Ok(count)
}

fn append_complement_segment<'slot>(
    output: &mut ChargedVec<ScalarRange>,
    ranges: &[ScalarRange],
    segment: ScalarRange,
    budget: &RegexExecutionBudget,
    mut out: Out<'slot, '_, RegexCompileError>,
) -> OResult<'slot, ()> {
    let mut cursor = segment.start;
    for range in ranges {
        budget.consume(1, out.reborrow())?;
        if range.end < segment.start || range.start > segment.end {
            continue;
        }
        let start = range.start.max(segment.start);
        let end = range.end.min(segment.end);
        if cursor < start {
            output.try_push(
                ScalarRange {
                    start: cursor,
                    end: start - 1,
                },
                out.reborrow(),
            )?;
        }
        if end >= cursor {
            cursor = end.saturating_add(1);
        }
    }
    if cursor <= segment.end {
        output.try_push(
            ScalarRange {
                start: cursor,
                end: segment.end,
            },
            out.reborrow(),
        )?;
    }
    Ok(())
}
