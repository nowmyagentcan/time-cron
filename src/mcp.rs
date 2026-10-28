//! The MCP server on stdio: one tool per command, answering with the CLI's `--json` objects.

use std::sync::Arc;

use rmcp::handler::server::tool::{schema_for_input, schema_for_output};
use rmcp::model::{CallToolResult, ContentBlock, JsonObject};
use rmcp::service::ServerInitializeError;
use rmcp::{ServerHandler, ServiceExt, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde_json::Value;

use crate::error::Error;
use crate::report::{self, ConvertArgs, Converted, Explained, NextArgs, Source, Upcoming};

#[derive(Clone)]
struct Server;

#[tool_router]
impl Server {
    #[tool(
        description = "Explain a cron expression in plain English and flag likely mistakes. Reads Unix cron (5 fields), Quartz (6-7 fields, seconds first) and AWS EventBridge cron(...) (6 fields with year); the dialect is detected unless given. Returns the description, each field's meaning and warnings.",
        input_schema = input::<Source>(),
        output_schema = schema_for_output::<Explained>(),
        annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    fn explain_cron(&self, args: JsonObject) -> CallToolResult {
        call("explain", args)
    }

    #[tool(
        description = "List the next run times of a cron expression in an IANA time zone, following the scheduler's own daylight-saving rules (cronie for Unix, Quartz, EventBridge Scheduler). Returns times with offsets and in UTC, runs moved or skipped by DST, and warnings.",
        input_schema = input::<NextArgs>(),
        output_schema = schema_for_output::<Upcoming>(),
        annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    fn next_cron_runs(&self, args: JsonObject) -> CallToolResult {
        call("next", args)
    }

    #[tool(
        description = "Convert a cron expression between Unix cron, Quartz and AWS EventBridge. Returns one expression, two when the target needs separate day-of-month and day-of-week triggers, or an error saying what the target can't express.",
        input_schema = input::<ConvertArgs>(),
        output_schema = schema_for_output::<Converted>(),
        annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    fn convert_cron(&self, args: JsonObject) -> CallToolResult {
        call("convert", args)
    }
}

#[tool_handler(
    name = "time-cron",
    instructions = "Explain cron expressions, list their next run times in any time zone with each scheduler's daylight-saving rules, and convert between Unix cron, Quartz and AWS EventBridge. Use these tools instead of computing cron schedules yourself."
)]
impl ServerHandler for Server {}

fn input<T: JsonSchema + 'static>() -> Arc<JsonObject> {
    schema_for_input::<T>().expect("tool arguments are objects")
}

/// The answer as structured content, or the error object as an error result. Errors carry
/// no structured content, so clients don't check them against the output schema.
fn call(op: &str, mut args: JsonObject) -> CallToolResult {
    args.insert("op".into(), op.into());
    match report::answer(Value::Object(args)) {
        Ok(answer) => CallToolResult::structured(answer),
        Err(error) => CallToolResult::error(vec![ContentBlock::text(report::failure(&error))]),
    }
}

/// Serves until the client closes stdin.
pub fn serve() -> Result<(), Error> {
    let io = |e: &dyn std::fmt::Display| Error::new("io", format!("Can't read input: {e}."));
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| io(&e))?;
    runtime.block_on(async {
        match Server.serve(rmcp::transport::stdio()).await {
            Ok(service) => service.waiting().await.map(drop).map_err(|e| io(&e)),
            // stdin closed before the handshake ended: the client left, which is no failure.
            Err(ServerInitializeError::ConnectionClosed(_)) => Ok(()),
            // stdout closed before the answer went out: the client left too.
            Err(ServerInitializeError::TransportError { error, .. })
                if error
                    .error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::BrokenPipe) =>
            {
                Ok(())
            }
            Err(e) => Err(io(&e)),
        }
    })
}
