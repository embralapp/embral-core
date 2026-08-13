//! Seeds staged dictation rows into a library database: dev fixture
//! tooling for scratch libraries (the app writes real rows only at the end
//! of a live dictation, which needs a microphone). Rows carry explicit
//! timestamps and dedupe on raw_text, so reruns are no-ops.
//!
//!   cargo run -p embral-db --example seed_dictations -- <db> <rows.json> [--reset]
//!
//! rows.json: [{ raw_text, cleaned_text, app, created_at (RFC 3339) }, ...]
//! Insert oldest-first: the history lists by row id descending, so id
//! order must match date order. --reset clears the table first.
//! Run only while no app instance holds the database.

use serde::Deserialize;

#[derive(Deserialize)]
struct Row {
    raw_text: String,
    cleaned_text: Option<String>,
    app: Option<String>,
    created_at: String,
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let (Some(db), Some(json)) = (args.next(), args.next()) else {
        anyhow::bail!("usage: seed_dictations <db> <rows.json>");
    };
    let mut rows: Vec<Row> = serde_json::from_str(&std::fs::read_to_string(json)?)?;
    rows.sort_by(|a, b| a.created_at.cmp(&b.created_at));

    let conn = rusqlite::Connection::open(db)?;
    if std::env::args().any(|a| a == "--reset") {
        conn.execute("DELETE FROM dictations", [])?;
    }
    let mut inserted = 0usize;
    for r in &rows {
        inserted += conn.execute(
            "INSERT INTO dictations (raw_text, cleaned_text, app, created_at)
             SELECT ?1, ?2, ?3, ?4
             WHERE NOT EXISTS (SELECT 1 FROM dictations WHERE raw_text = ?1)",
            rusqlite::params![r.raw_text, r.cleaned_text, r.app, r.created_at],
        )?;
    }
    println!("seeded {inserted} dictation row(s) ({} given)", rows.len());
    Ok(())
}
