//! The tools as rmcp publishes them: names, descriptions for the model,
//! schemas and annotations, and each call handed to the matching method of
//! [`McpServer`] with the [`Caller`] the HTTP layer authenticated. Nothing
//! is decided here.

use std::sync::Arc;

use rmcp::handler::server::tool::{schema_for_input, schema_for_output};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolResult, ContentBlock, Implementation, JsonObject, ProgressNotificationParam, ServerCapabilities,
    ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData, RoleServer, ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value as Json;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::{
    CallError, Caller, ClientInfo, DescribeTableArgs, DescribeTableOutput, ExecuteArgs, ExecuteOutput, INSTRUCTIONS,
    ListConnectionsArgs, ListConnectionsOutput, ListSchemasArgs, ListSchemasOutput, ListTablesArgs, ListTablesOutput,
    McpServer, Progress, QueryArgs, QueryOutput,
};

const LIST_CONNECTIONS: &str = "List the database connections the user lets this client use in IdeDB: each one's \
                                id, name, engine, database, and whether it allows writes. Start here: the other \
                                tools take a connection's id (or its name, when unique) as `connection`. A \
                                connection with `available: false` can't be used; `note` says why.";
const LIST_SCHEMAS: &str = "List a connection's schemas (Postgres schemas, MySQL databases, SQLite attached \
                            databases) and the default one, where unqualified names resolve. Engine catalogs such \
                            as pg_catalog or information_schema are left out unless `includeSystem` is true.";
const LIST_TABLES: &str = "List the tables and views of a schema, with their kind and comment. Without `schema`, \
                           the connection's default schema. Use describe_table for their columns.";
const DESCRIBE_TABLE: &str = "Describe a table or view: its columns (type, nullability, default, position in the \
                              primary key, generated, comment), its foreign keys, and the foreign keys of other \
                              tables in the same schema that reference it. Look before writing SQL against it.";
const QUERY: &str = "Run one read-only SQL statement (SELECT, WITH, SHOW, EXPLAIN…) on a read-only session and \
                     return its rows as JSON: 200 rows unless `maxRows` says otherwise (up to 1000), with \
                     `truncated` set when there were more. A statement that writes is refused: use execute. One \
                     statement per call; no transaction control or session settings.";
const EXECUTE: &str = "Run one SQL statement that changes data or schema (INSERT, UPDATE, DELETE, CREATE, ALTER, \
                       DROP…). The user must approve each statement in IdeDB, so the call waits for them (2 minutes \
                       by default) and they may reject it: pass `reason` to tell them why it should run. Refused \
                       without asking where this client may only read, or on a connection marked never-write. A read \
                       sent here runs right away.";

/// What rmcp serves: one per session, or per request without one.
#[derive(Clone)]
pub(crate) struct Handler {
    server: McpServer,
}

impl Handler {
    pub fn new(server: McpServer) -> Self {
        Self { server }
    }
}

#[tool_router]
impl Handler {
    #[tool(
        name = "list_connections",
        title = "List connections",
        description = LIST_CONNECTIONS,
        input_schema = input_schema::<ListConnectionsArgs>(),
        output_schema = output_schema::<ListConnectionsOutput>(),
        annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    async fn list_connections(
        &self,
        _: Parameters<ListConnectionsArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller(&context)?;
        reply(self.server.list_connections(&caller).await)
    }

    #[tool(
        name = "list_schemas",
        title = "List schemas",
        description = LIST_SCHEMAS,
        input_schema = input_schema::<ListSchemasArgs>(),
        output_schema = output_schema::<ListSchemasOutput>(),
        annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    async fn list_schemas(
        &self,
        Parameters(args): Parameters<ListSchemasArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller(&context)?;
        reply(self.server.list_schemas(&caller, args).await)
    }

    #[tool(
        name = "list_tables",
        title = "List tables",
        description = LIST_TABLES,
        input_schema = input_schema::<ListTablesArgs>(),
        output_schema = output_schema::<ListTablesOutput>(),
        annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    async fn list_tables(
        &self,
        Parameters(args): Parameters<ListTablesArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller(&context)?;
        reply(self.server.list_tables(&caller, args).await)
    }

    #[tool(
        name = "describe_table",
        title = "Describe a table",
        description = DESCRIBE_TABLE,
        input_schema = input_schema::<DescribeTableArgs>(),
        output_schema = output_schema::<DescribeTableOutput>(),
        annotations(read_only_hint = true, idempotent_hint = true, open_world_hint = false)
    )]
    async fn describe_table(
        &self,
        Parameters(args): Parameters<DescribeTableArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller(&context)?;
        reply(self.server.describe_table(&caller, args).await)
    }

    #[tool(
        name = "query",
        title = "Run a read-only query",
        description = QUERY,
        input_schema = input_schema::<QueryArgs>(),
        output_schema = output_schema::<QueryOutput>(),
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn query(
        &self,
        Parameters(args): Parameters<QueryArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller(&context)?;
        reply(self.server.query(&caller, args, &context.ct).await)
    }

    #[tool(
        name = "execute",
        title = "Run a statement the user approves",
        description = EXECUTE,
        input_schema = input_schema::<ExecuteArgs>(),
        output_schema = output_schema::<ExecuteOutput>(),
        annotations(read_only_hint = false, destructive_hint = true, idempotent_hint = false, open_world_hint = false)
    )]
    async fn execute(
        &self,
        Parameters(args): Parameters<ExecuteArgs>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let caller = caller(&context)?;
        let (progress, relayed) = relay_progress(&context);
        let outcome = self.server.execute(&caller, args, progress, &context.ct).await;
        // Every report reaches the client before the result does.
        if let Some(relayed) = relayed {
            let _ = relayed.await;
        }
        reply(outcome)
    }
}

#[tool_handler]
impl ServerHandler for Handler {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("IdeDB", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }
}

/// The caller the HTTP layer authenticated, completed with what this
/// request declares about the client: in its `_meta` (protocol 2026-07-28),
/// or else in the `initialize` of its session, which rmcp keeps.
fn caller(context: &RequestContext<RoleServer>) -> Result<Caller, ErrorData> {
    let parts = context.extensions.get::<http::request::Parts>();
    let Some(mut caller) = parts.and_then(|parts| parts.extensions.get::<Caller>()).cloned() else {
        return Err(ErrorData::internal_error("IdeDB received a tool call without an authenticated client", None));
    };
    // Outside a session rmcp has no `initialize` to fall back on, and would
    // offer its own name instead.
    let in_session = parts.is_some_and(|parts| parts.headers.contains_key("mcp-session-id"));
    let declared = context.meta.client_info().or_else(|| {
        let session = context.peer.peer_info().filter(|_| in_session)?;
        Some(session.client_info.clone())
    });
    caller.client_info = declared
        .map(|info| ClientInfo { name: info.name, version: Some(info.version).filter(|version| !version.is_empty()) });
    caller.protocol_version = context.protocol_version().map(|version| version.to_string());
    Ok(caller)
}

/// A call's result for the model: its JSON as structured content and as
/// text. A tool error is a result the model reads (`isError`); an internal
/// error is a JSON-RPC error.
fn reply<T: Serialize>(outcome: Result<T, CallError>) -> Result<CallToolResult, ErrorData> {
    match outcome {
        Ok(output) => match serde_json::to_value(output) {
            Ok(json) => Ok(CallToolResult::structured(json)),
            Err(e) => Err(ErrorData::internal_error(format!("IdeDB could not encode the result: {e}"), None)),
        },
        Err(CallError::Tool(error)) => Ok(CallToolResult::error(vec![ContentBlock::text(error.message)])),
        Err(CallError::Internal(error)) => Err(ErrorData::internal_error(error.to_string(), None)),
    }
}

/// A tool's arguments schema: rmcp's, from the core's type.
fn input_schema<T: JsonSchema + 'static>() -> Arc<JsonObject> {
    let schema = schema_for_input::<T>().unwrap_or_else(|e| panic!("{}: {e}", std::any::type_name::<T>()));
    portable(&schema)
}

/// A tool's result schema: rmcp's, from the core's type.
fn output_schema<T: JsonSchema + 'static>() -> Arc<JsonObject> {
    portable(&schema_for_output::<T>())
}

/// `schema` without the integer formats schemars adds (`uint64`…): JSON
/// Schema defines none of them, and clients' validators warn about or
/// refuse them. The bounds they imply stay, as `minimum` and `maximum`.
fn portable(schema: &JsonObject) -> Arc<JsonObject> {
    fn strip(value: &mut Json) {
        match value {
            Json::Object(object) => {
                let format = object.get("format").and_then(Json::as_str);
                if format.is_some_and(|format| NUMBER_FORMATS.contains(&format)) {
                    object.remove("format");
                }
                object.values_mut().for_each(strip);
            }
            Json::Array(items) => items.iter_mut().for_each(strip),
            _ => {}
        }
    }
    const NUMBER_FORMATS: [&str; 12] =
        ["int", "int8", "int16", "int32", "int64", "uint", "uint8", "uint16", "uint32", "uint64", "float", "double"];
    let mut schema = Json::Object(schema.clone());
    strip(&mut schema);
    let Json::Object(schema) = schema else { unreachable!("still an object") };
    Arc::new(schema)
}

/// A progress callback for the core that sends each report to the client as
/// a progress notification, when the request asked for them with a
/// `progressToken`; and the task sending them, which ends once the callback
/// is dropped and every report was sent.
fn relay_progress(
    context: &RequestContext<RoleServer>,
) -> (impl Fn(Progress) + Send + Sync + 'static, Option<JoinHandle<()>>) {
    let (sender, relayed) = match context.meta.get_progress_token() {
        None => (None, None),
        Some(token) => {
            let (sender, mut reports) = mpsc::unbounded_channel::<Progress>();
            let peer = context.peer.clone();
            let relayed = tokio::spawn(async move {
                while let Some(report) = reports.recv().await {
                    let notification = ProgressNotificationParam::new(token.clone(), report.waited_secs as f64)
                        .with_total(report.timeout_secs as f64)
                        .with_message(report.message);
                    // Fails only once the client is gone, and nobody waits for it.
                    let _ = peer.notify_progress(notification).await;
                }
            });
            (Some(sender), Some(relayed))
        }
    };
    let callback = move |report| {
        if let Some(sender) = &sender {
            let _ = sender.send(report);
        }
    };
    (callback, relayed)
}

#[cfg(test)]
mod tests {
    use rmcp::model::ErrorCode;
    use serde_json::{Value as Json, json};

    use super::*;
    use crate::{Error, ToolError};

    fn text(result: &CallToolResult) -> &str {
        &result.content[0].as_text().expect("text content").text
    }

    #[test]
    fn results_carry_their_json_as_structured_content_and_as_text() {
        let output = ListSchemasOutput { default_schema: Some("public".into()), schemas: Vec::new() };
        let result = reply(Ok(output)).unwrap();
        assert_eq!(result.is_error, Some(false));
        let structured = result.structured_content.clone().unwrap();
        assert_eq!(structured, json!({ "defaultSchema": "public", "schemas": [] }));
        assert_eq!(serde_json::from_str::<Json>(text(&result)).unwrap(), structured);
    }

    #[test]
    fn tool_errors_are_results_and_internal_errors_are_protocol_errors() {
        let refused = reply::<QueryOutput>(Err(ToolError::new("Use the execute tool.").into())).unwrap();
        assert_eq!((refused.is_error, text(&refused)), (Some(true), "Use the execute tool."));
        assert_eq!(refused.structured_content, None);

        let failed = reply::<QueryOutput>(Err(Error::Internal("the store is locked".into()).into())).unwrap_err();
        assert_eq!((failed.code, failed.message.as_ref()), (ErrorCode::INTERNAL_ERROR, "the store is locked"));
    }

    #[test]
    fn schemas_keep_only_formats_json_schema_defines() {
        let schema = json!({
            "type": "object",
            "properties": {
                "rowCount": { "type": "integer", "format": "uint64", "minimum": 0 },
                "maxRows": { "type": ["integer", "null"], "format": "uint32", "minimum": 1, "maximum": 1000 },
                "at": { "type": "string", "format": "date-time" },
                "format": { "type": "string" },
            },
            "$defs": { "Column": { "anyOf": [{ "type": "number", "format": "double" }] } },
        });
        let Json::Object(schema) = schema else { unreachable!() };
        let expected = json!({
            "type": "object",
            "properties": {
                "rowCount": { "type": "integer", "minimum": 0 },
                "maxRows": { "type": ["integer", "null"], "minimum": 1, "maximum": 1000 },
                "at": { "type": "string", "format": "date-time" },
                "format": { "type": "string" },
            },
            "$defs": { "Column": { "anyOf": [{ "type": "number" }] } },
        });
        assert_eq!(Json::Object(portable(&schema).as_ref().clone()), expected);

        let query = Json::Object(input_schema::<QueryArgs>().as_ref().clone());
        assert_eq!(query["properties"]["maxRows"]["maximum"], 1000, "{query}");
        assert!(!query.to_string().contains("\"format\""), "{query}");
    }
}
