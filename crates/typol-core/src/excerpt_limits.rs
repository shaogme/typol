/// Independent source excerpt limits, including record and text capacities.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExcerptLimits {
    max_bytes: usize,
    max_line_bytes: usize,
    max_records: usize,
    context_lines: usize,
}

impl ExcerptLimits {
    pub const fn new(
        max_bytes: usize,
        max_line_bytes: usize,
        max_records: usize,
        context_lines: usize,
    ) -> Self {
        Self {
            max_bytes,
            max_line_bytes,
            max_records,
            context_lines,
        }
    }

    pub const fn max_bytes(&self) -> usize {
        self.max_bytes
    }

    pub const fn max_line_bytes(&self) -> usize {
        self.max_line_bytes
    }

    pub const fn max_records(&self) -> usize {
        self.max_records
    }

    /// Total extra context lines per span, split evenly before and after.
    pub const fn context_lines(&self) -> usize {
        self.context_lines
    }

    pub const fn with_max_bytes(mut self, value: usize) -> Self {
        self.max_bytes = value;
        self
    }

    pub const fn with_max_line_bytes(mut self, value: usize) -> Self {
        self.max_line_bytes = value;
        self
    }

    pub const fn with_max_records(mut self, value: usize) -> Self {
        self.max_records = value;
        self
    }

    pub const fn with_context_lines(mut self, value: usize) -> Self {
        self.context_lines = value;
        self
    }

    pub const fn enabled(&self) -> bool {
        self.max_bytes > 0 && self.max_line_bytes > 0 && self.max_records > 0
    }

    pub(crate) fn omission(&self) -> &'static str {
        if self.max_line_bytes >= "…".len() {
            "…"
        } else {
            "."
        }
    }

    pub(crate) fn first_line(&self, line: usize) -> usize {
        line.saturating_sub(self.context_lines / 2).max(1)
    }

    pub(crate) const fn last_line(&self, line: usize) -> usize {
        line.saturating_add(self.context_lines - self.context_lines / 2)
    }
}

impl Default for ExcerptLimits {
    fn default() -> Self {
        Self::new(65_536, 512, 128, 2)
    }
}
