//! Selects a source reader while keeping feed-specific parsing out of preprocessing and routing.
pub mod gtfs;
pub mod matsim;
pub mod matsim_config;
use anyhow::Result;
use flate2::read::GzDecoder;
use std::{
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
};
/// Stream compressed XML without loading the entire schedule in memory.
pub fn xml_input(path: &Path) -> Result<Box<dyn BufRead>> {
    let file = File::open(path)?;
    if path.extension().is_some_and(|e| e == "gz") {
        Ok(Box::new(BufReader::new(GzDecoder::new(file))))
    } else {
        Ok(Box::new(BufReader::new(file)))
    }
}
