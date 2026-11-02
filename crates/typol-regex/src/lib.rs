#![no_std]

extern crate alloc;

mod char_class;
mod compiler;
mod error;
mod matcher;
mod parser;

pub use compiler::{Regex, RegexCompileOptions, RegexCompiler};
pub use error::{
    PatternSpan, RegexCompileError, RegexErrorContext, RegexMatchError, RegexResource,
    RegexScratchError, RegexSyntaxError, RegexSyntaxErrorKind,
};
pub use matcher::RegexScratch;
pub use opeo;
pub use typol_policy::RegexLimits;
