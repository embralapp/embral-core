//! The MCP surface: eight read-only tools over the library, served by rmcp.
//! Query logic lives in [`crate::queries`]; this file is schemas, glue,
//! and the result envelope. Search is hybrid (keyword + semantic) when the
//! embedding model is on disk and keyword-only otherwise, silently: a
//! search never errors because semantics are unavailable.

use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars, tool, tool_handler, tool_router, ErrorData as McpError, ServerHandler,
};
use serde_json::{json, Value};

use crate::queries::{self, SearchParams};
use crate::store::{EmbedderSlot, Store, ToolError};

const INSTRUCTIONS: &str = "Read-only access to the user's local embral library: meeting \
transcripts, AI summaries, the user's own notes, and dictation history. Start with \
search_meetings (or search_dictations for personal voice notes) — results are evidence, \
each with an opaque passage_id, its meeting, speakers, and timestamps. Expand a promising \
hit with get_passage_context instead of pulling whole transcripts. Filters: `participants` \
= people who were IN the meeting, `speakers` = people who SAID the words; asking about a \
person needs neither — just put them in the query. Meeting ids come from list/search \
results. Pasted images are fetchable: get_meeting lists them, image-sourced hits carry \
an `image` field, and get_meeting_image returns the picture itself (downscaled to a \
client-safe size) with its OCR text. If tools fail, call get_storage_status to see why.";

#[derive(Clone)]
pub struct EmbralServer {
    store: std::sync::Arc<Store>,
    embedder: std::sync::Arc<EmbedderSlot>,
    tool_router: ToolRouter<Self>,
}

#[derive(Clone, Copy, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SearchMode {
    /// Quoted text goes exact, everything else hybrid (the default).
    Auto,
    /// Keyword + semantic, fused.
    Hybrid,
    /// Verbatim words only.
    Exact,
    /// Meaning only, no keyword leg.
    Semantic,
}

impl SearchMode {
    fn to_mode(self) -> embral_search::Mode {
        match self {
            SearchMode::Auto => embral_search::Mode::Auto,
            SearchMode::Hybrid => embral_search::Mode::Hybrid,
            SearchMode::Exact => embral_search::Mode::Exact,
            SearchMode::Semantic => embral_search::Mode::Semantic,
        }
    }
}

#[derive(Clone, Copy, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceArg {
    Transcript,
    UserNotes,
    Summary,
    ImageText,
}

impl SourceArg {
    fn to_source(self) -> embral_search::Source {
        match self {
            SourceArg::Transcript => embral_search::Source::Transcript,
            SourceArg::UserNotes => embral_search::Source::UserNotes,
            SourceArg::Summary => embral_search::Source::Summary,
            SourceArg::ImageText => embral_search::Source::ImageText,
        }
    }
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SearchMeetingsArgs {
    /// What to find: a natural-language description, topic, or exact
    /// phrase (put exact phrases in double quotes).
    pub query: String,
    pub mode: Option<SearchMode>,
    /// Restrict where passages come from: verbatim speech ("transcript"),
    /// the user's own notes ("user_notes"), AI summaries ("summary"), or
    /// text read out of images the user pasted into a meeting, such as
    /// screenshots of slides, whiteboards, diagrams ("image_text").
    pub sources: Option<Vec<SourceArg>>,
    /// Only meetings these people were in (attendee names). "Meetings Jane
    /// attended" filters here; "what Jane said" belongs in `speakers`;
    /// "discussions about Jane" is just the query.
    pub participants: Option<Vec<String>>,
    /// Only passages these people spoke (transcript speaker names).
    pub speakers: Option<Vec<String>>,
    /// RFC3339 date-time or YYYY-MM-DD; meetings at or after this.
    pub after: Option<String>,
    /// RFC3339 date-time or YYYY-MM-DD (whole day included).
    pub before: Option<String>,
    /// Max passages, 1-25 (default 8).
    pub limit: Option<u32>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct SearchDictationsArgs {
    /// What to find in the dictation history.
    pub query: String,
    pub mode: Option<SearchMode>,
    /// RFC3339 date-time or YYYY-MM-DD; dictations at or after this.
    pub after: Option<String>,
    /// RFC3339 date-time or YYYY-MM-DD (whole day included).
    pub before: Option<String>,
    /// Max hits, 1-25 (default 8).
    pub limit: Option<u32>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct PassageContextArgs {
    /// A passage_id from a search result. Ids are transient: they change
    /// when a meeting is edited; re-search rather than storing them.
    pub passage_id: i64,
    /// Transcript passages: seconds of surrounding speech before (default 60).
    pub before_secs: Option<f64>,
    /// Transcript passages: seconds of surrounding speech after (default 60).
    pub after_secs: Option<f64>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct MeetingArgs {
    /// The meeting id, as returned by list_meetings or search_meetings.
    pub id: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct MeetingImageArgs {
    /// The meeting id, as returned by list_meetings or search_meetings.
    pub id: String,
    /// The image filename, from get_meeting's `images` list or a search
    /// hit's `image` field (e.g. "img-01.png").
    pub image: String,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct TranscriptArgs {
    /// The meeting id.
    pub id: String,
    /// Render only from this many seconds in (omit for the whole document).
    pub from_secs: Option<f64>,
    /// Render only up to this many seconds in.
    pub to_secs: Option<f64>,
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
pub struct ListArgs {
    /// Maximum meetings to return, 1-100 (default 20).
    pub limit: Option<u32>,
    /// Only meetings started at or after this RFC3339 date-time or date.
    pub since: Option<String>,
    /// Only meetings this person attended (attendee name).
    pub participant: Option<String>,
}

/// `{ok:true, …}` on success; `{ok:false, error:{code,message}}` as an
/// execution error (never a protocol error) so the model can react.
fn respond(result: Result<Value, ToolError>) -> Result<CallToolResult, McpError> {
    let (payload, failed) = match result {
        Ok(Value::Object(mut map)) => {
            map.insert("ok".into(), Value::Bool(true));
            (Value::Object(map), false)
        }
        Ok(other) => (json!({ "ok": true, "result": other }), false),
        Err(e) => (
            json!({ "ok": false, "error": { "code": e.code(), "message": e.message() } }),
            true,
        ),
    };
    let text = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string());
    Ok(if failed {
        CallToolResult::error(vec![ContentBlock::text(text)])
    } else {
        CallToolResult::success(vec![ContentBlock::text(text)])
    })
}

#[tool_router]
impl EmbralServer {
    pub fn new(store: Store) -> Self {
        Self {
            store: store.into(),
            embedder: std::sync::Arc::new(EmbedderSlot::default()),
            tool_router: Self::tool_router(),
        }
    }

    /// The semantic leg's query vector: absent in exact mode, when the
    /// model isn't downloaded, or when inference fails (all degrade to
    /// keyword-only, silently).
    fn query_vector(&self, query: &str, mode: Option<SearchMode>) -> Option<Vec<f32>> {
        if matches!(mode, Some(SearchMode::Exact)) {
            return None;
        }
        self.embedder.embed_query(query)
    }

    #[tool(
        description = "Report where the embral library lives and whether it is readable and searchable: storage folder, database presence, schema versions, meeting count, and the semantic index's state. Call this when other tools fail.",
        annotations(read_only_hint = true)
    )]
    async fn get_storage_status(&self) -> Result<CallToolResult, McpError> {
        respond(queries::storage_status(&self.store, self.embedder.loaded()))
    }

    #[tool(
        description = "Search the user's meetings — transcripts, their own notes, and AI summaries — by meaning and keywords. Returns ranked passages with passage_id, meeting, speakers, and timestamps. `participants` filters to meetings people attended; `speakers` to words people said; questions ABOUT a person need neither. Prefer this over fetching documents.",
        annotations(read_only_hint = true)
    )]
    async fn search_meetings(
        &self,
        Parameters(args): Parameters<SearchMeetingsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let vector = self.query_vector(&args.query, args.mode);
        let params = SearchParams {
            query: args.query,
            mode: args
                .mode
                .map(SearchMode::to_mode)
                .unwrap_or(embral_search::Mode::Auto),
            sources: args
                .sources
                .map(|s| s.into_iter().map(SourceArg::to_source).collect()),
            participants: args.participants,
            speakers: args.speakers,
            after: args.after,
            before: args.before,
            limit: args.limit,
        };
        respond(
            self.store
                .open()
                .and_then(|db| queries::search_meetings(&db, &params, vector.as_deref())),
        )
    }

    #[tool(
        description = "Search the user's dictation history — personal voice notes dictated into other apps, separate from meetings. Use for 'what did I dictate/note about ...' questions.",
        annotations(read_only_hint = true)
    )]
    async fn search_dictations(
        &self,
        Parameters(args): Parameters<SearchDictationsArgs>,
    ) -> Result<CallToolResult, McpError> {
        let vector = self.query_vector(&args.query, args.mode);
        let params = SearchParams {
            query: args.query,
            mode: args
                .mode
                .map(SearchMode::to_mode)
                .unwrap_or(embral_search::Mode::Auto),
            sources: None,
            participants: None,
            speakers: None,
            after: args.after,
            before: args.before,
            limit: args.limit,
        };
        respond(
            self.store
                .open()
                .and_then(|db| queries::search_dictations(&db, &params, vector.as_deref())),
        )
    }

    #[tool(
        description = "Expand a search hit: transcript passages return the surrounding minutes of speech; note and summary passages return their neighboring passages. Use this on promising hits instead of fetching whole transcripts.",
        annotations(read_only_hint = true)
    )]
    async fn get_passage_context(
        &self,
        Parameters(args): Parameters<PassageContextArgs>,
    ) -> Result<CallToolResult, McpError> {
        respond(self.store.open().and_then(|db| {
            queries::passage_context(
                &db,
                args.passage_id,
                args.before_secs.unwrap_or(60.0),
                args.after_secs.unwrap_or(60.0),
            )
        }))
    }

    #[tool(
        description = "One meeting in full context: metadata, who attended vs who actually spoke, the AI summary document, and the user's own notes.",
        annotations(read_only_hint = true)
    )]
    async fn get_meeting(
        &self,
        Parameters(args): Parameters<MeetingArgs>,
    ) -> Result<CallToolResult, McpError> {
        respond(
            self.store
                .open()
                .and_then(|db| queries::get_meeting(&db, &self.store.storage_dir, &args.id)),
        )
    }

    #[tool(
        description = "One pasted image from a meeting, as viewable image content plus its OCR text. Filenames come from get_meeting's `images` list or a search hit's `image` field. Large images are downscaled to a client-safe size; originals stay untouched on disk.",
        annotations(read_only_hint = true)
    )]
    async fn get_meeting_image(
        &self,
        Parameters(args): Parameters<MeetingImageArgs>,
    ) -> Result<CallToolResult, McpError> {
        let fetched = self
            .store
            .open()
            .and_then(|db| crate::images::fetch(&self.store.storage_dir, &db, &args.id, &args.image));
        match fetched {
            // The one tool whose success is not a single text envelope:
            // the picture must be a real image block or clients will not
            // show it to their model. The metadata sits beside it.
            Ok(f) => {
                use base64::Engine;
                let data = base64::engine::general_purpose::STANDARD.encode(&f.bytes);
                let mut meta = serde_json::Map::new();
                meta.insert("ok".into(), json!(true));
                meta.insert("meeting_id".into(), json!(args.id));
                meta.insert("image".into(), json!(args.image));
                meta.insert("mime_type".into(), json!(f.mime));
                meta.insert("width".into(), json!(f.width));
                meta.insert("height".into(), json!(f.height));
                meta.insert("scaled".into(), json!(f.scaled));
                if let Some(text) = &f.image_text {
                    meta.insert("image_text".into(), json!(text));
                }
                let meta_text = serde_json::to_string_pretty(&Value::Object(meta))
                    .unwrap_or_default();
                Ok(CallToolResult::success(vec![
                    ContentBlock::image(data, f.mime),
                    ContentBlock::text(meta_text),
                ]))
            }
            Err(e) => respond(Err(e)),
        }
    }

    #[tool(
        description = "A meeting's transcript — the whole document, or a from_secs/to_secs window of it. Can return a lot of text; find the right spot with search_meetings and get_passage_context first.",
        annotations(read_only_hint = true)
    )]
    async fn get_transcript(
        &self,
        Parameters(args): Parameters<TranscriptArgs>,
    ) -> Result<CallToolResult, McpError> {
        respond(
            self.store.open().and_then(|db| {
                queries::get_transcript(&db, &args.id, args.from_secs, args.to_secs)
            }),
        )
    }

    #[tool(
        description = "List the user's meetings, newest first, with id, title, start time, duration, and attendees. Use `since` for recency and `participant` for one person's meetings.",
        annotations(read_only_hint = true)
    )]
    async fn list_meetings(
        &self,
        Parameters(args): Parameters<ListArgs>,
    ) -> Result<CallToolResult, McpError> {
        respond(self.store.open().and_then(|db| {
            queries::list_meetings(
                &db,
                args.limit,
                args.since.as_deref(),
                args.participant.as_deref(),
            )
        }))
    }
}

// Route through the field built once in `new` rather than the macro's
// default `Self::tool_router()`, which would rebuild the router per call.
#[tool_handler(router = self.tool_router)]
impl ServerHandler for EmbralServer {
    fn get_info(&self) -> ServerInfo {
        let mut server_info = Implementation::default();
        server_info.name = "embral".into();
        server_info.version = env!("CARGO_PKG_VERSION").into();
        let mut info = ServerInfo::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = server_info;
        info.instructions = Some(INSTRUCTIONS.into());
        info
    }
}
