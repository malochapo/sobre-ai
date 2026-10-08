//! Structured installation-health report for the dashboard doctor signal (#466).
//!
//! `sobre doctor`'s terminal renderer ([`super::run`]) emits ANSI-coloured
//! lines that are unfit for JSON. This module re-derives the same pass/fail
//! predicates that [`super::compact_score`] counts into a clean, serializable
//! shape the dashboard renders as a three-level health badge
//! (good / warnings / issues) with a per-check breakdown — without shelling out
//! or scraping coloured stdout, so the CLI and the dashboard stay in lockstep
//! from a single source of truth.
