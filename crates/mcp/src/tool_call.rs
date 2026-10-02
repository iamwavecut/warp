use std::future::Future;
use std::time::Duration;

use futures::FutureExt;
use futures::future::{Either, select};
use rmcp::model::{
    CallToolRequest, CallToolRequestParams, CallToolResult, CancelledNotificationParam,
    ClientRequest, ErrorData, ServerResult,
};
use rmcp::service::PeerRequestOptions;
use rmcp::{Peer, RoleClient, ServiceError};
use warpui::r#async::Timer;

/// Total connection, dispatch, and response budget for one tool call.
pub const TOOL_CALL_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const CANCELLATION_TIMEOUT: Duration = Duration::from_secs(5);

/// Calls a tool once, preserving transport resumption but bounding the logical request.
pub async fn call_tool_with_deadline(
    params: CallToolRequestParams,
    connect: impl Future<Output = Result<Peer<RoleClient>, ServiceError>>,
) -> Result<CallToolResult, ServiceError> {
    call_tool_with_deadline_inner(
        params,
        connect,
        Timer::after(TOOL_CALL_TIMEOUT).map(|_| ()),
        || Timer::after(CANCELLATION_TIMEOUT).map(|_| ()),
    )
    .await
}

async fn call_tool_with_deadline_inner<Connect, Deadline, CancellationDeadline, CancellationTimer>(
    params: CallToolRequestParams,
    connect: Connect,
    deadline: Deadline,
    cancellation_deadline: CancellationDeadline,
) -> Result<CallToolResult, ServiceError>
where
    Connect: Future<Output = Result<Peer<RoleClient>, ServiceError>>,
    Deadline: Future<Output = ()>,
    CancellationDeadline: FnOnce() -> CancellationTimer,
    CancellationTimer: Future<Output = ()>,
{
    let mut dispatch_started = false;
    let mut cancellation = None;

    let completed = {
        let operation = async {
            let peer = connect.await?;
            dispatch_started = true;
            let request = ClientRequest::CallToolRequest(CallToolRequest::new(params));
            // rmcp's own timeout awaits cancellation delivery before returning. Keep the
            // logical deadline outside that path so a stalled send cannot defeat it.
            let handle = peer
                .send_cancellable_request(request, PeerRequestOptions::no_options())
                .await?;
            cancellation = Some((peer, handle.id.clone()));
            match handle.await_response().await? {
                ServerResult::CallToolResult(result) => Ok(result),
                _ => Err(ServiceError::UnexpectedResponse),
            }
        };
        match select(Box::pin(operation), Box::pin(deadline)).await {
            Either::Left((result, _)) => Some(result),
            Either::Right(((), _)) => None,
        }
    };

    if let Some(result) = completed {
        return result;
    }

    if let Some((peer, request_id)) = cancellation {
        let cancel = peer.notify_cancelled(CancelledNotificationParam::new(
            Some(request_id),
            Some("Warp MCP tool call deadline exceeded".to_owned()),
        ));
        let _ = select(Box::pin(cancel), Box::pin(cancellation_deadline())).await;
    }

    let message = if !dispatch_started {
        format!(
            "MCP tool call timed out after {} seconds while connecting. The tool was not dispatched.",
            TOOL_CALL_TIMEOUT.as_secs()
        )
    } else {
        format!(
            "MCP tool call deadline exceeded after {} seconds without a result. The tool may have executed; its outcome is unknown. Do not retry automatically.",
            TOOL_CALL_TIMEOUT.as_secs()
        )
    };
    Err(ServiceError::McpError(ErrorData::internal_error(
        message, None,
    )))
}

#[cfg(test)]
#[path = "tool_call_tests.rs"]
mod tests;
