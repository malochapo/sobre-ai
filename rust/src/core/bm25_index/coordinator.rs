//! Non-blocking, single-flight build coordinator for the dashboard (#452).
//!
//! Mirrors `graph_index::coordinator`: the dashboard's search routes must not
//! block the request thread on a full BM25 build (expensive on large repos).
//! A fresh on-disk index is returned immediately via the cheap sentinel
//! staleness check; otherwise a single background build is started and the
//! caller gets `Err(progress)` so the handler can answer `202 Accepted`.
