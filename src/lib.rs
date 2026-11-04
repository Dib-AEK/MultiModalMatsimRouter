//! Provides a source-neutral, immutable transit timetable and reusable parallel routing engine.
pub mod batching;
pub mod cache;
pub mod config;
pub mod cost;
pub mod engine;
pub mod model;
pub mod preprocess;
pub mod readers;
pub mod routing;
pub mod search_index;
pub mod spatial;
pub mod time;
pub mod weighted;
pub mod work_history;
