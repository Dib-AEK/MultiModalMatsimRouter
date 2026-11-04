//! Parses service-day times into compact milliseconds, including overnight and fractional values.
use anyhow::{Result, bail, ensure};
pub type Time = u32;
pub const NEVER: Time = u32::MAX;

pub fn parse_time(text: &str) -> Result<Time> {
    let parts: Vec<_> = text.split(':').collect();
    ensure!(parts.len() == 3, "time must be HH:MM:SS[.sss]: {text}");
    let hours: u64 = parts[0].parse()?;
    let minutes: u64 = parts[1].parse()?;
    let seconds: f64 = parts[2].parse()?;
    ensure!(
        minutes < 60 && seconds.is_finite() && (0.0..60.0).contains(&seconds),
        "invalid time: {text}"
    );
    let millis = hours
        .checked_mul(3_600_000)
        .and_then(|h| h.checked_add(minutes * 60_000))
        .and_then(|m| m.checked_add((seconds * 1000.0).round() as u64));
    match millis {
        Some(n) if n < NEVER as u64 => Ok(n as Time),
        _ => bail!("time out of range: {text}"),
    }
}

pub fn format_time(time: Time) -> String {
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        time / 3_600_000,
        time / 60_000 % 60,
        time / 1000 % 60,
        time % 1000
    )
}

pub fn seconds(value: f64) -> Result<Time> {
    ensure!(
        value.is_finite() && value >= 0.0 && value * 1000.0 < NEVER as f64,
        "invalid duration: {value}"
    );
    let rounded = (value * 1000.0).ceil();
    ensure!(
        rounded < NEVER as f64,
        "duration collides with unreachable sentinel"
    );
    Ok(rounded as Time)
}
