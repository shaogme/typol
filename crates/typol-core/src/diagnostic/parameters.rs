use super::kind::DiagnosticFieldStatus;
use alloc::{borrow::Cow, string::String};
use core::fmt;
use u18n::RenderError;

pub const MAX_DIAGNOSTIC_PARAMETERS: usize = 4;
pub const MAX_DIAGNOSTIC_PARAMETER_NAME_BYTES: usize = 64;

#[derive(Clone, Copy, Eq, PartialEq)]
pub enum DiagnosticParameterValue {
    Unsigned(usize),
}

impl fmt::Debug for DiagnosticParameterValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Unsigned(<redacted>)")
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub struct DiagnosticParameter {
    name: &'static str,
    value: DiagnosticParameterValue,
}

impl fmt::Debug for DiagnosticParameter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DiagnosticParameter")
            .field("name", &self.name)
            .field("value", &self.value)
            .finish()
    }
}

impl DiagnosticParameter {
    pub const fn unsigned(name: &'static str, value: usize) -> Self {
        Self {
            name,
            value: DiagnosticParameterValue::Unsigned(value),
        }
    }

    pub const fn name(self) -> &'static str {
        self.name
    }

    pub const fn value(self) -> DiagnosticParameterValue {
        self.value
    }
}

#[derive(Clone, Copy)]
pub struct DiagnosticParameters {
    values: [DiagnosticParameter; MAX_DIAGNOSTIC_PARAMETERS],
    length: usize,
    status: DiagnosticFieldStatus,
}

impl DiagnosticParameters {
    pub const fn empty() -> Self {
        Self {
            values: [DiagnosticParameter::unsigned("", 0); MAX_DIAGNOSTIC_PARAMETERS],
            length: 0,
            status: DiagnosticFieldStatus::NotApplicable,
        }
    }

    pub const fn unavailable() -> Self {
        Self {
            values: [DiagnosticParameter::unsigned("", 0); MAX_DIAGNOSTIC_PARAMETERS],
            length: 0,
            status: DiagnosticFieldStatus::Unavailable,
        }
    }

    pub fn new(values: [Option<DiagnosticParameter>; MAX_DIAGNOSTIC_PARAMETERS]) -> Self {
        let mut parameters = Self::empty();
        let mut ended = false;
        for (index, value) in values.into_iter().enumerate() {
            match value {
                Some(value)
                    if !ended
                        && index == parameters.length
                        && !value.name().is_empty()
                        && value.name().len() <= MAX_DIAGNOSTIC_PARAMETER_NAME_BYTES
                        && !parameters.contains_name(value.name()) =>
                {
                    let Some(stored) = parameters.values.get_mut(index) else {
                        let mut unavailable = Self::empty();
                        unavailable.status = DiagnosticFieldStatus::Unavailable;
                        return unavailable;
                    };
                    *stored = value;
                    parameters.length += 1;
                }
                Some(_) => {
                    let mut unavailable = Self::empty();
                    unavailable.status = DiagnosticFieldStatus::Unavailable;
                    return unavailable;
                }
                None => ended = true,
            }
        }
        if parameters.length > 0 {
            parameters.status = DiagnosticFieldStatus::Available;
        }
        parameters
    }

    pub fn as_slice(&self) -> &[DiagnosticParameter] {
        self.values.get(..self.length).unwrap_or(&[])
    }

    pub const fn len(&self) -> usize {
        self.length
    }

    pub const fn is_empty(&self) -> bool {
        self.length == 0
    }

    pub const fn status(&self) -> DiagnosticFieldStatus {
        self.status
    }

    fn contains_name(&self, name: &str) -> bool {
        self.values
            .iter()
            .take(self.length)
            .any(|parameter| parameter_names_equal(parameter.name(), name))
    }
}

fn parameter_names_equal(left: &str, right: &str) -> bool {
    left.len() == right.len() && left.bytes().zip(right.bytes()).all(|(a, b)| a == b)
}

impl fmt::Debug for DiagnosticParameters {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DiagnosticParameters")
            .field("count", &self.length)
            .field("status", &self.status)
            .finish()
    }
}

#[derive(Clone)]
pub struct DiagnosticMessage<'a> {
    text: Cow<'a, str>,
    parameters: Option<&'a DiagnosticParameters>,
    failure: Option<RenderError>,
}

impl<'a> DiagnosticMessage<'a> {
    /// Creates a message that is displayed as provided.
    pub const fn custom(text: &'a str) -> Self {
        Self {
            text: Cow::Borrowed(text),
            parameters: None,
            failure: None,
        }
    }

    pub const fn with_parameters(text: &'a str, parameters: &'a DiagnosticParameters) -> Self {
        Self {
            text: Cow::Borrowed(text),
            parameters: Some(parameters),
            failure: None,
        }
    }

    /// Returns the final message text.
    pub fn text(&self) -> &str {
        self.text.as_ref()
    }

    pub const fn parameters(&self) -> Option<&'a DiagnosticParameters> {
        self.parameters
    }

    pub const fn failure(&self) -> Option<RenderError> {
        self.failure
    }
}

impl From<Result<Cow<'static, str>, RenderError>> for DiagnosticMessage<'static> {
    fn from(message: Result<Cow<'static, str>, RenderError>) -> Self {
        match message {
            Ok(text) => Self {
                text,
                parameters: None,
                failure: None,
            },
            Err(error) => Self {
                text: Cow::Borrowed(""),
                parameters: None,
                failure: Some(error),
            },
        }
    }
}

impl<'a> From<&'a str> for DiagnosticMessage<'a> {
    fn from(text: &'a str) -> Self {
        Self::custom(text)
    }
}

impl<'a> From<&'a String> for DiagnosticMessage<'a> {
    fn from(text: &'a String) -> Self {
        Self::custom(text)
    }
}
