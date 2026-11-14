use opeo::{OResult, Out};
use typol_i18n::t;

use super::{
    super::{DisplayError, renderer::FormatOutput},
    MessageTranslator,
};
use crate::{HostError, HostErrorKind, HostOperation};

pub(super) struct HostErrorRenderer;

impl HostErrorRenderer {
    pub(super) fn write_localized<'slot>(
        error: &HostError,
        output: &mut impl FormatOutput,
        localizer: &mut MessageTranslator,
        mut out: Out<'slot, '_, DisplayError>,
    ) -> OResult<'slot, ()> {
        let translator = localizer.translator();
        let text = t! { name: "report.host_operation", locale: translator };
        localizer.write_translated(text, output, out.reborrow())?;
        output.append(" ", out.reborrow())?;
        let text = match error.operation() {
            HostOperation::SchemaRead => {
                t! { name: "report.host_operation.schema_read", locale: translator }
            }
            HostOperation::ConfigRead => {
                t! { name: "report.host_operation.config_read", locale: translator }
            }
            HostOperation::SourceNames => {
                t! { name: "report.host_operation.source_names", locale: translator }
            }
        };
        localizer.write_translated(text, output, out.reborrow())?;
        output.append(" ", out.reborrow())?;
        let text = t! { name: "report.failed", locale: translator };
        localizer.write_translated(text, output, out.reborrow())?;
        output.append(" (", out.reborrow())?;
        output.append(error.context().failure_point(), out.reborrow())?;
        if let Some(source_id) = error.source_id() {
            output.append(", ", out.reborrow())?;
            let text = t! { name: "report.source_id", locale: translator };
            localizer.write_translated(text, output, out.reborrow())?;
            output.append(" ", out.reborrow())?;
            output.number(source_id.get() as usize, out.reborrow())?;
        }
        if let Some(offset) = error.offset() {
            output.append(", ", out.reborrow())?;
            let text = t! { name: "report.offset", locale: translator };
            localizer.write_translated(text, output, out.reborrow())?;
            output.append(" ", out.reborrow())?;
            output.number(offset, out.reborrow())?;
        }
        output.append("): ", out.reborrow())?;
        let text = match error.kind() {
            HostErrorKind::NotFound => {
                t! { name: "report.host_kind.not_found", locale: translator }
            }
            HostErrorKind::PermissionDenied => {
                t! { name: "report.host_kind.permission_denied", locale: translator }
            }
            HostErrorKind::NotRegularFile => {
                t! { name: "report.host_kind.not_regular_file", locale: translator }
            }
            HostErrorKind::InvalidRequest => {
                t! { name: "report.host_kind.invalid_request", locale: translator }
            }
            HostErrorKind::ReadFailure => {
                t! { name: "report.host_kind.read_failure", locale: translator }
            }
            HostErrorKind::ResourceLimitExceeded => {
                t! { name: "report.host_kind.resource_limit_exceeded", locale: translator }
            }
            HostErrorKind::AllocationFailed => {
                t! { name: "report.host_kind.allocation_failed", locale: translator }
            }
            HostErrorKind::CapacityExceeded => {
                t! { name: "report.host_kind.capacity_exceeded", locale: translator }
            }
            HostErrorKind::InternalError => {
                t! { name: "report.host_kind.internal_error", locale: translator }
            }
        };
        localizer.write_translated(text, output, out.reborrow())
    }
}
