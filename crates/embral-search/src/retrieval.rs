//! Hybrid retrieval: an FTS5 leg and a vector leg fused by reciprocal-rank
//! fusion, with multiplicative boosts. The caller supplies the query vector
//! (or doesn't; no model means the FTS leg alone, silently: search never
//! errors because semantics are unavailable).

use anyhow::Result;
use chrono::{DateTime, SecondsFormat, Utc};
use embral_db::rusqlite::types::Value;
use embral_db::rusqlite::Connection;
use embral_db::Db;

use crate::chunker::Source;
use crate::vector::{SqliteVecIndex, VectorIndex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Auto,
    Hybrid,
    Exact,
    Semantic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerKind {
    Meetings,
    Dictations,
}

pub struct SearchArgs<'a> {
    pub query: &'a str,
    pub mode: Mode,
    pub owner: OwnerKind,
    pub limit: usize,
    pub sources: Option<Vec<Source>>,
    /// People in the meeting (attendee display names).
    pub participants: Option<Vec<String>>,
    /// People who said it (chunk speaker labels).
    pub speakers: Option<Vec<String>>,
    pub after: Option<DateTime<Utc>>,
    pub before: Option<DateTime<Utc>>,
    /// Live typing: the word under the cursor matches as a prefix.
    pub prefix_last_token: bool,
}

impl<'a> SearchArgs<'a> {
    pub fn new(query: &'a str, owner: OwnerKind) -> Self {
        SearchArgs {
            query,
            mode: Mode::Auto,
            owner,
            limit: 10,
            sources: None,
            participants: None,
            speakers: None,
            after: None,
            before: None,
            prefix_last_token: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub chunk_id: i64,
    pub meeting_id: Option<String>,
    pub dictation_id: Option<i64>,
    pub title: Option<String>,
    pub date: DateTime<Utc>,
    pub source: String,
    pub speakers: Vec<String>,
    pub start_secs: Option<f64>,
    pub end_secs: Option<f64>,
    pub text: String,
    /// Which image this passage was read out of; `Some` only for
    /// `image_text`. There is nothing in the document to scroll to for an
    /// image hit, so this is what a caller points at.
    pub image_filename: Option<String>,
    /// FTS-provided excerpt with `[bracketed]` match markers; absent for
    /// hits the vector leg alone surfaced.
    pub snippet: Option<String>,
    pub fts_rank: Option<usize>,
    pub semantic_rank: Option<usize>,
    pub exact_phrase: bool,
    pub score: f64,
}

const RRF_K: f64 = 60.0;
const BOOST_PHRASE: f64 = 1.5;
const BOOST_TITLE: f64 = 1.25;
const BOOST_USER_NOTES: f64 = 1.15;
const BOOST_SPEAKER: f64 = 1.2;
const RECENCY_STRENGTH: f64 = 0.3;
const RECENCY_HALF_LIFE_DAYS: f64 = 14.0;

fn rrf(rank: usize) -> f64 {
    1.0 / (RRF_K + rank as f64)
}

fn tokens(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// A quoted substring means the user wants those words verbatim.
fn quoted_phrase(query: &str) -> Option<String> {
    let start = query.find('"')?;
    let end = query[start + 1..].find('"')? + start + 1;
    let phrase = query[start + 1..end].trim();
    (!phrase.is_empty()).then(|| phrase.to_string())
}

fn wants_recency(query: &str) -> bool {
    let toks = tokens(query);
    let cues = ["recent", "recently", "latest", "last", "yesterday", "today", "newest"];
    toks.iter().any(|t| cues.contains(&t.as_str()))
        || query.to_lowercase().contains("this week")
}

/// (resolved mode, the phrase for exact matching when one applies)
fn resolve_mode(mode: Mode, query: &str) -> (Mode, Option<String>) {
    match mode {
        Mode::Auto => match quoted_phrase(query) {
            Some(p) => (Mode::Exact, Some(p)),
            None => (Mode::Hybrid, None),
        },
        Mode::Exact => {
            let p = quoted_phrase(query).unwrap_or_else(|| query.trim().to_string());
            (Mode::Exact, Some(p))
        }
        other => (other, quoted_phrase(query)),
    }
}

fn fts_quote(token: &str) -> String {
    format!("\"{}\"", token.replace('"', "\"\""))
}

/// Free text → an FTS5 expression of quoted words (implicit AND), so `-`,
/// `*`, `"` or `NEAR` in user input match literally instead of parsing as
/// operators.
///
/// **With `prefix_last`, the last word is a prefix.** Live search runs
/// while the user is still typing, so the final token is almost never a
/// whole word: matching it exactly meant results appeared only when a
/// word happened to be complete and vanished on the next keystroke (the
/// palette's old "search is slow" bug, which was really a gap). Earlier
/// tokens are words the user finished, and stay exact. The `*` goes
/// outside the quotes: FTS5 reads `"integratio" *` as a prefix query,
/// while `"integratio*"` would search for a literal asterisk.
fn fts_expr(query: &str, phrase: Option<&str>, prefix_last: bool) -> String {
    if let Some(p) = phrase {
        return fts_quote(p);
    }
    let words: Vec<&str> = query.split_whitespace().collect();
    let last = words.len().saturating_sub(1);
    words
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let quoted = fts_quote(w);
            if prefix_last && i == last {
                format!("{quoted} *")
            } else {
                quoted
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn rfc3339(dt: &DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// The WHERE-clause tail shared by both legs (everything but the MATCH),
/// with its parameters. `?1` is reserved for the leg's own first parameter.
struct Filters {
    sql: String,
    params: Vec<Value>,
}

fn build_filters(args: &SearchArgs, first_free_param: usize) -> Filters {
    let mut sql = String::new();
    let mut params: Vec<Value> = Vec::new();
    let mut n = first_free_param;
    let mut push = |params: &mut Vec<Value>, v: Value| -> String {
        params.push(v);
        let placeholder = format!("?{n}");
        n += 1;
        placeholder
    };

    match args.owner {
        OwnerKind::Meetings => sql.push_str(" AND c.meeting_id IS NOT NULL"),
        OwnerKind::Dictations => sql.push_str(" AND c.dictation_id IS NOT NULL"),
    }
    if let Some(sources) = &args.sources {
        if !sources.is_empty() {
            let placeholders: Vec<String> = sources
                .iter()
                .map(|s| push(&mut params, Value::from(s.as_str().to_string())))
                .collect();
            sql.push_str(&format!(" AND c.source IN ({})", placeholders.join(", ")));
        }
    }
    if let Some(people) = &args.participants {
        if !people.is_empty() && args.owner == OwnerKind::Meetings {
            let placeholders: Vec<String> = people
                .iter()
                .map(|p| push(&mut params, Value::from(p.to_lowercase())))
                .collect();
            sql.push_str(&format!(
                " AND EXISTS (SELECT 1 FROM json_each(m.attendees) je
                              WHERE lower(je.value) IN ({}))",
                placeholders.join(", ")
            ));
        }
    }
    if let Some(people) = &args.speakers {
        if !people.is_empty() {
            let placeholders: Vec<String> = people
                .iter()
                .map(|p| push(&mut params, Value::from(p.to_lowercase())))
                .collect();
            sql.push_str(&format!(
                " AND EXISTS (SELECT 1 FROM json_each(c.speakers) js
                              WHERE lower(js.value) IN ({}))",
                placeholders.join(", ")
            ));
        }
    }
    let date_col = match args.owner {
        OwnerKind::Meetings => "m.started_at",
        OwnerKind::Dictations => "d.created_at",
    };
    if let Some(after) = &args.after {
        let ph = push(&mut params, Value::from(rfc3339(after)));
        sql.push_str(&format!(" AND {date_col} >= {ph}"));
    }
    if let Some(before) = &args.before {
        let ph = push(&mut params, Value::from(rfc3339(before)));
        sql.push_str(&format!(" AND {date_col} <= {ph}"));
    }

    Filters { sql, params }
}

const JOINS: &str = " FROM chunks c
       LEFT JOIN meetings m ON m.id = c.meeting_id
       LEFT JOIN dictations d ON d.id = c.dictation_id ";

/// FTS candidates in bm25 order, with snippets.
fn fts_leg(
    conn: &Connection,
    args: &SearchArgs,
    expr: &str,
    pool: usize,
) -> Result<Vec<(i64, String)>> {
    let filters = build_filters(args, 3);
    let sql = format!(
        "SELECT c.id, snippet(chunks_fts, 0, '[', ']', ' … ', 12)
         FROM chunks_fts
         JOIN chunks c ON c.id = chunks_fts.rowid
         LEFT JOIN meetings m ON m.id = c.meeting_id
         LEFT JOIN dictations d ON d.id = c.dictation_id
         WHERE chunks_fts MATCH ?1{}
         ORDER BY bm25(chunks_fts)
         LIMIT ?2",
        filters.sql
    );
    let mut all_params: Vec<Value> = vec![Value::from(expr.to_string()), Value::from(pool as i64)];
    all_params.extend(filters.params);
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(
            embral_db::rusqlite::params_from_iter(all_params),
            |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
        )?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// KNN candidates that survive the filters, preserving distance order.
fn vector_leg(
    conn: &Connection,
    args: &SearchArgs,
    query_vector: &[f32],
    k: usize,
) -> Result<Vec<i64>> {
    let knn = SqliteVecIndex.knn(conn, query_vector, k)?;
    if knn.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<String> = knn.iter().map(|(id, _)| id.to_string()).collect();
    let filters = build_filters(args, 1);
    let sql = format!(
        "SELECT c.id{JOINS} WHERE c.id IN ({}){}",
        ids.join(", "),
        filters.sql
    );
    let mut stmt = conn.prepare(&sql)?;
    let allowed: std::collections::HashSet<i64> = stmt
        .query_map(embral_db::rusqlite::params_from_iter(filters.params), |r| {
            r.get::<_, i64>(0)
        })?
        .collect::<std::result::Result<_, _>>()?;
    Ok(knn
        .into_iter()
        .map(|(id, _)| id)
        .filter(|id| allowed.contains(id))
        .collect())
}

struct Candidate {
    meeting_id: Option<String>,
    dictation_id: Option<i64>,
    title: Option<String>,
    date: DateTime<Utc>,
    source: String,
    speakers: Vec<String>,
    start_secs: Option<f64>,
    end_secs: Option<f64>,
    text: String,
    image_filename: Option<String>,
}

fn load_candidates(
    conn: &Connection,
    ids: &[i64],
) -> Result<std::collections::HashMap<i64, Candidate>> {
    if ids.is_empty() {
        return Ok(Default::default());
    }
    let id_list: Vec<String> = ids.iter().map(|i| i.to_string()).collect();
    let sql = format!(
        "SELECT c.id, c.meeting_id, c.dictation_id, c.source, c.text, c.speakers,
                c.start_secs, c.end_secs, m.title, m.started_at, d.created_at,
                c.image_filename
         {JOINS} WHERE c.id IN ({})",
        id_list.join(", ")
    );
    let mut stmt = conn.prepare(&sql)?;
    let mut out = std::collections::HashMap::new();
    let rows = stmt.query_map([], |r| {
        let id: i64 = r.get(0)?;
        let started_at: Option<String> = r.get(9)?;
        let created_at: Option<String> = r.get(10)?;
        Ok((
            id,
            (
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
                r.get::<_, Option<f64>>(6)?,
                r.get::<_, Option<f64>>(7)?,
                r.get::<_, Option<String>>(8)?,
                started_at.or(created_at),
                r.get::<_, Option<String>>(11)?,
            ),
        ))
    })?;
    for row in rows {
        let (
            id,
            (
                meeting_id,
                dictation_id,
                source,
                text,
                speakers,
                start_secs,
                end_secs,
                title,
                date,
                image_filename,
            ),
        ) = row?;
        let date = date
            .and_then(|s| DateTime::parse_from_rfc3339(&s).ok())
            .map(|d| d.with_timezone(&Utc))
            .unwrap_or_else(Utc::now);
        out.insert(
            id,
            Candidate {
                meeting_id,
                dictation_id,
                title,
                date,
                source,
                speakers: serde_json::from_str(&speakers).unwrap_or_default(),
                start_secs,
                end_secs,
                text,
                image_filename,
            },
        );
    }
    Ok(out)
}

/// The pure scoring core: RRF over the two rank lists, then boosts.
/// Exposed for unit tests with injected rank orders.
pub(crate) fn fuse_score(
    fts_rank: Option<usize>,
    semantic_rank: Option<usize>,
    w_fts: f64,
    w_vec: f64,
    exact_phrase: bool,
    title_match: bool,
    is_user_notes: bool,
    speaker_match: bool,
    recency: Option<f64 /* age in days */>,
) -> f64 {
    let mut score = w_fts * fts_rank.map(rrf).unwrap_or(0.0)
        + w_vec * semantic_rank.map(rrf).unwrap_or(0.0);
    if exact_phrase {
        score *= BOOST_PHRASE;
    }
    if title_match {
        score *= BOOST_TITLE;
    }
    if is_user_notes {
        score *= BOOST_USER_NOTES;
    }
    if speaker_match {
        score *= BOOST_SPEAKER;
    }
    if let Some(age_days) = recency {
        score *= 1.0 + RECENCY_STRENGTH * (-age_days / RECENCY_HALF_LIFE_DAYS).exp();
    }
    score
}

pub fn search(db: &Db, args: &SearchArgs, query_vector: Option<&[f32]>) -> Result<Vec<Hit>> {
    let query = args.query.trim();
    if query.is_empty() || args.limit == 0 {
        return Ok(Vec::new());
    }
    let (mut mode, phrase) = resolve_mode(args.mode, query);
    // No query vector means no semantic leg exists; keyword results beat
    // no results, and the caller shouldn't have to know.
    if query_vector.is_none() && mode == Mode::Semantic {
        mode = Mode::Hybrid;
    }
    let recency_armed = wants_recency(query);

    let fts_pool = std::cmp::max(50, 5 * args.limit);
    let knn_k = std::cmp::min(200, 8 * args.limit);

    let (w_fts, w_vec, run_fts, run_vec) = match mode {
        Mode::Exact => (1.0, 0.0, true, false),
        Mode::Semantic => (0.0, 1.0, false, true),
        _ => (1.0, 1.0, true, true),
    };

    db.with_conn(|conn| {
        let fts_hits: Vec<(i64, String)> = if run_fts {
            fts_leg(conn, args, &fts_expr(query, phrase.as_deref(), args.prefix_last_token), fts_pool)?
        } else {
            Vec::new()
        };
        let vec_hits: Vec<i64> = match (run_vec, query_vector) {
            (true, Some(v)) => vector_leg(conn, args, v, knn_k)?,
            _ => Vec::new(),
        };

        let fts_ranks: std::collections::HashMap<i64, (usize, String)> = fts_hits
            .into_iter()
            .enumerate()
            .map(|(i, (id, snip))| (id, (i + 1, snip)))
            .collect();
        let vec_ranks: std::collections::HashMap<i64, usize> = vec_hits
            .iter()
            .enumerate()
            .map(|(i, &id)| (id, i + 1))
            .collect();

        let union: Vec<i64> = {
            let mut ids: Vec<i64> = fts_ranks.keys().copied().collect();
            for id in vec_ranks.keys() {
                if !fts_ranks.contains_key(id) {
                    ids.push(*id);
                }
            }
            ids
        };
        let candidates = load_candidates(conn, &union)?;

        let query_tokens = tokens(query);
        let phrase_needle = phrase
            .clone()
            .or_else(|| (query_tokens.len() >= 2).then(|| query.to_string()))
            .map(|p| p.to_lowercase());
        let now = Utc::now();

        let mut hits: Vec<Hit> = union
            .into_iter()
            .filter_map(|id| {
                let c = candidates.get(&id)?;
                let fts = fts_ranks.get(&id);
                let semantic_rank = vec_ranks.get(&id).copied();
                let exact_phrase = phrase_needle
                    .as_deref()
                    .map(|p| c.text.to_lowercase().contains(p))
                    .unwrap_or(false);
                let title_match = c
                    .title
                    .as_deref()
                    .map(|t| {
                        let title = t.to_lowercase();
                        !query_tokens.is_empty()
                            && query_tokens.iter().all(|tok| title.contains(tok))
                    })
                    .unwrap_or(false);
                let speaker_match = c.speakers.iter().any(|s| {
                    tokens(s).iter().any(|st| query_tokens.contains(st))
                });
                let age_days = (now - c.date).num_hours() as f64 / 24.0;
                let score = fuse_score(
                    fts.map(|(r, _)| *r),
                    semantic_rank,
                    w_fts,
                    w_vec,
                    exact_phrase,
                    title_match,
                    c.source == "user_notes",
                    speaker_match,
                    recency_armed.then_some(age_days.max(0.0)),
                );
                Some(Hit {
                    chunk_id: id,
                    meeting_id: c.meeting_id.clone(),
                    dictation_id: c.dictation_id,
                    title: c.title.clone(),
                    date: c.date,
                    source: c.source.clone(),
                    speakers: c.speakers.clone(),
                    start_secs: c.start_secs,
                    end_secs: c.end_secs,
                    text: c.text.clone(),
                    image_filename: c.image_filename.clone(),
                    snippet: fts.map(|(_, s)| s.clone()),
                    fts_rank: fts.map(|(r, _)| *r),
                    semantic_rank,
                    exact_phrase,
                    score,
                })
            })
            .collect();

        hits.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        hits.truncate(args.limit);
        Ok(hits)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::test_support::*;
    use crate::index::{store_embeddings, sync_dictation, sync_meeting};
    use crate::model;
    use chrono::TimeZone;

    fn library() -> Db {
        let db = Db::open_in_memory().unwrap();

        let mut budget = meeting("m-budget", "Budget Review", 1, &["Alice", "Bob"]);
        budget.summary = "# Budget Review\n\n## Key Takeaways\n\nSpending freeze until Q4.".into();
        db.upsert_meeting(&budget).unwrap();
        db.replace_segments(
            "m-budget",
            &[
                seg("Alice", "The quarterly budget needs a spending freeze.", 0.0),
                seg("Bob", "Marketing takes the largest cut this quarter.", 20.0),
            ],
        )
        .unwrap();
        db.set_notes("m-budget", "freeze confirmed by finance").unwrap();

        let hiring = meeting("m-hiring", "Hiring Sync", 20, &["Alice", "Dana"]);
        db.upsert_meeting(&hiring).unwrap();
        db.replace_segments(
            "m-hiring",
            &[seg("Dana", "Two engineering offers go out this week.", 0.0)],
        )
        .unwrap();

        let dictation_id = db
            .add_dictation("remind me to send the budget spreadsheet", None, None)
            .unwrap();

        sync_meeting(&db, "m-budget").unwrap();
        sync_meeting(&db, "m-hiring").unwrap();
        sync_dictation(&db, dictation_id).unwrap();
        db
    }

    fn hits(db: &Db, args: &SearchArgs) -> Vec<Hit> {
        search(db, args, None).unwrap()
    }

    #[test]
    fn fts_search_finds_and_snippets() {
        let db = library();
        let found = hits(&db, &SearchArgs::new("budget", OwnerKind::Meetings));
        assert!(!found.is_empty());
        assert!(found[0].snippet.as_deref().unwrap().contains("[budget]"));
        assert!(found.iter().all(|h| h.meeting_id.is_some()));
        // The dictation mentioning "budget" is not in the meetings corpus.
        assert!(found.iter().all(|h| h.dictation_id.is_none()));

        let dictated = hits(&db, &SearchArgs::new("budget", OwnerKind::Dictations));
        assert_eq!(dictated.len(), 1);
        assert_eq!(dictated[0].dictation_id, Some(1));
    }

    #[test]
    fn filters_narrow_the_corpus() {
        let db = library();

        // Speaker filter: who said it.
        let mut args = SearchArgs::new("quarter", OwnerKind::Meetings);
        args.speakers = Some(vec!["Bob".into()]);
        let found = hits(&db, &args);
        assert!(!found.is_empty());
        assert!(found.iter().all(|h| h.speakers.contains(&"Bob".to_string())));

        // Participant filter: who was there. Dana does say "offers" (in
        // m-hiring), but she isn't in m-budget, so "budget" + Dana = empty.
        let mut args = SearchArgs::new("budget", OwnerKind::Meetings);
        args.participants = Some(vec!["dana".into()]);
        assert!(hits(&db, &args).is_empty());

        // Source filter.
        let mut args = SearchArgs::new("freeze", OwnerKind::Meetings);
        args.sources = Some(vec![Source::UserNotes]);
        let found = hits(&db, &args);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].source, "user_notes");

        // Date filter: only the June-20 meeting is after June 10.
        let mut args = SearchArgs::new("offers", OwnerKind::Meetings);
        args.after = Some(chrono::Utc.with_ymd_and_hms(2026, 6, 10, 0, 0, 0).unwrap());
        assert_eq!(hits(&db, &args).len(), 1);
        args.before = Some(chrono::Utc.with_ymd_and_hms(2026, 6, 11, 0, 0, 0).unwrap());
        assert!(hits(&db, &args).is_empty());
    }

    /// Live search runs while a word is still being typed: every stroke
    /// must find the meeting, finished words stay exact, and the prefix is
    /// not a free-for-all. (Ported from the old meeting-level search when
    /// it died in v8; the lesson predates this engine.)
    #[test]
    fn prefix_matching_serves_live_typing() {
        let db = library();
        let mut args = SearchArgs::new("spend", OwnerKind::Meetings);
        assert!(hits(&db, &args).is_empty(), "whole-word match misses the prefix");
        args.prefix_last_token = true;
        assert!(!hits(&db, &args).is_empty());

        for partial in ["quart", "quarterly bu", "quarterly budget"] {
            let mut args = SearchArgs::new(partial, OwnerKind::Meetings);
            args.prefix_last_token = true;
            assert!(!hits(&db, &args).is_empty(), "typing {partial:?} should find it");
        }
        // Earlier words must still match; only the cursor word is loose.
        let mut args = SearchArgs::new("zzz quart", OwnerKind::Meetings);
        args.prefix_last_token = true;
        assert!(hits(&db, &args).is_empty());
    }

    /// FTS operator syntax in user input matches literally, never errors.
    /// (Also ported from the old meeting-level search.)
    #[test]
    fn search_survives_fts_operator_input() {
        let db = library();
        for q in ["\"unbalanced", "a AND OR NOT", "col:evil", "x*", "-", "  "] {
            let mut args = SearchArgs::new(q, OwnerKind::Meetings);
            args.prefix_last_token = true;
            search(&db, &args, None).unwrap();
        }
    }

    #[test]
    fn quoted_queries_go_exact() {
        let db = library();
        let found = hits(&db, &SearchArgs::new("\"spending freeze\"", OwnerKind::Meetings));
        assert!(!found.is_empty());
        assert!(found[0].exact_phrase);
        // The phrase in a different order matches nothing.
        assert!(hits(&db, &SearchArgs::new("\"freeze spending\"", OwnerKind::Meetings)).is_empty());
    }

    #[test]
    fn the_vector_leg_joins_the_fusion() {
        let db = library();
        db.with_conn(|conn| SqliteVecIndex.ensure(conn)).unwrap();

        // Hand-placed vectors for exactly two transcript chunks: hiring at
        // e1, one budget chunk at e2; no distance ties, no boost noise.
        let ids: Vec<(i64, String)> = db
            .with_conn(|conn| {
                let mut stmt = conn.prepare(
                    "SELECT id, text FROM chunks WHERE source = 'transcript'",
                )?;
                let rows = stmt
                    .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .unwrap();
        let axis = |hot: bool| {
            let mut v = vec![0.0f32; model::DIM];
            v[if hot { 0 } else { 1 }] = 1.0;
            v
        };
        let rows: Vec<(i64, Vec<f32>)> = ids
            .iter()
            .map(|(id, text)| (*id, axis(text.contains("offers"))))
            .collect();
        store_embeddings(&db, &rows).unwrap();

        // A query vector at e1 must surface the hiring chunk even though
        // the query text shares no keywords with it.
        let mut query_vec = vec![0.0f32; model::DIM];
        query_vec[0] = 1.0;
        let args = SearchArgs::new("recruiting pipeline", OwnerKind::Meetings);
        let found = search(&db, &args, Some(&query_vec)).unwrap();
        assert!(!found.is_empty());
        assert!(found[0].text.contains("offers"));
        assert!(found[0].semantic_rank.is_some());
        assert!(found[0].fts_rank.is_none());
        assert!(found[0].snippet.is_none()); // semantic-only hits carry no FTS excerpt
    }

    #[test]
    fn fusion_math_and_boosts() {
        // Both-legs beats either single leg at equal ranks.
        let both = fuse_score(Some(1), Some(1), 1.0, 1.0, false, false, false, false, None);
        let fts_only = fuse_score(Some(1), None, 1.0, 1.0, false, false, false, false, None);
        let vec_only = fuse_score(None, Some(1), 1.0, 1.0, false, false, false, false, None);
        assert!(both > fts_only && both > vec_only);
        assert!((both - (fts_only + vec_only)).abs() < 1e-12);
        assert!((fts_only - 1.0 / 61.0).abs() < 1e-12);

        // Each boost multiplies.
        let base = fuse_score(Some(2), None, 1.0, 1.0, false, false, false, false, None);
        assert!((fuse_score(Some(2), None, 1.0, 1.0, true, false, false, false, None) / base - 1.5).abs() < 1e-9);
        assert!((fuse_score(Some(2), None, 1.0, 1.0, false, true, false, false, None) / base - 1.25).abs() < 1e-9);
        assert!((fuse_score(Some(2), None, 1.0, 1.0, false, false, true, false, None) / base - 1.15).abs() < 1e-9);
        assert!((fuse_score(Some(2), None, 1.0, 1.0, false, false, false, true, None) / base - 1.2).abs() < 1e-9);

        // Recency decays with age and never goes below 1x.
        let fresh = fuse_score(Some(1), None, 1.0, 1.0, false, false, false, false, Some(0.0));
        let stale = fuse_score(Some(1), None, 1.0, 1.0, false, false, false, false, Some(365.0));
        assert!(fresh > stale);
        assert!(stale >= fts_only);
    }

    #[test]
    fn recency_cues_and_modes_resolve() {
        assert!(wants_recency("what did we decide last week"));
        assert!(wants_recency("the latest budget"));
        assert!(!wants_recency("a lasting impression")); // token, not substring

        assert_eq!(resolve_mode(Mode::Auto, "plain words").0, Mode::Hybrid);
        let (mode, phrase) = resolve_mode(Mode::Auto, "find \"single source of truth\" please");
        assert_eq!(mode, Mode::Exact);
        assert_eq!(phrase.as_deref(), Some("single source of truth"));
        let (_, phrase) = resolve_mode(Mode::Exact, "verbatim words");
        assert_eq!(phrase.as_deref(), Some("verbatim words"));
    }
}
