//! Where does hybrid search actually spend its time?
//!
//! A measurement, not a unit test, run by hand against a real library:
//!
//! ```text
//! EMBRAL_BENCH_DB=C:\Users\you\embral\embral.db \
//!   cargo test -p embral-search --lib bench -- --ignored --nocapture
//! ```
//!
//! The vector leg is timed with a synthetic query vector: KNN cost doesn't
//! care where the vector came from. Query-embedding latency is measured
//! by embral-embedder's ignored test; the palette budget (~30 ms per
//! keystroke) is judged against both numbers together, recorded in
//! storage.md (measured, not assumed, per the repo's search history).

mod tests {
    use std::time::Instant;

    use crate::retrieval::{search, Mode, OwnerKind, SearchArgs};
    use embral_db::Db;

    fn ms(start: Instant) -> f64 {
        start.elapsed().as_secs_f64() * 1000.0
    }

    #[test]
    #[ignore = "measurement; needs EMBRAL_BENCH_DB pointing at a real library"]
    fn where_does_hybrid_search_spend_its_time() {
        let Ok(path) = std::env::var("EMBRAL_BENCH_DB") else {
            eprintln!("EMBRAL_BENCH_DB not set — nothing to measure");
            return;
        };
        let db = Db::open_read_only(std::path::Path::new(&path)).expect("open bench db");

        let (chunks, pending): (i64, i64) = db
            .with_conn(|conn| {
                Ok((
                    conn.query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get(0))?,
                    conn.query_row(
                        "SELECT COUNT(*) FROM chunks WHERE embedded_with IS NULL",
                        [],
                        |r| r.get(0),
                    )?,
                ))
            })
            .expect("count");
        eprintln!("library: {chunks} chunks, {pending} pending embeddings");

        // Any normalized vector exercises the KNN path identically.
        let mut synthetic = vec![0.0f32; crate::model::DIM];
        for (i, v) in synthetic.iter_mut().enumerate() {
            *v = ((i * 37 % 100) as f32 - 50.0) / 100.0;
        }
        let norm = synthetic.iter().map(|v| v * v).sum::<f32>().sqrt();
        synthetic.iter_mut().for_each(|v| *v /= norm);

        for query in ["budget", "what did we decide about hiring", "the", "\"next steps\""] {
            let mut args = SearchArgs::new(query, OwnerKind::Meetings);
            args.prefix_last_token = true;

            args.mode = Mode::Exact;
            let start = Instant::now();
            let fts_hits = search(&db, &args, None).expect("fts search").len();
            let fts_ms = ms(start);

            args.mode = Mode::Auto;
            let start = Instant::now();
            let hybrid_hits = search(&db, &args, Some(&synthetic)).expect("hybrid").len();
            let hybrid_ms = ms(start);

            eprintln!(
                "{query:40} fts: {fts_hits:3} hits {fts_ms:7.2} ms | hybrid: {hybrid_hits:3} hits {hybrid_ms:7.2} ms"
            );
        }
    }
}
