use std::str::FromStr;

use chrono::{DateTime, NaiveDateTime, Utc};

pub use crate::utils::secrets::read_secret;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("missing required configuration: {}", .0.join(", "))]
    Missing(Vec<&'static str>),
    #[error("{name} is not valid: {reason}")]
    Invalid { name: &'static str, reason: String },
}

pub type Lookup<'a> = &'a dyn Fn(&str) -> Option<String>;

pub fn value(lookup: Lookup<'_>, key: &str) -> Option<String> {
    lookup(key).map(|raw| raw.trim().to_owned()).filter(|raw| !raw.is_empty())
}

pub fn secret(lookup: Lookup<'_>, key: &str) -> Option<String> {
    read_secret(&|name| lookup(name), key).filter(|raw| !raw.is_empty())
}

pub fn required<const N: usize>(pairs: [(&'static str, Option<String>); N]) -> Result<[String; N], ConfigError> {
    let missing: Vec<&'static str> = pairs.iter().filter(|(_, v)| v.is_none()).map(|(name, _)| *name).collect();
    if !missing.is_empty() {
        return Err(ConfigError::Missing(missing));
    }
    Ok(pairs.map(|(_, v)| v.unwrap_or_default()))
}

pub fn parse_or<T: FromStr>(lookup: Lookup<'_>, name: &'static str, default: T) -> Result<T, ConfigError>
where
    T::Err: std::fmt::Display,
{
    value(lookup, name).map_or(Ok(default), |raw| {
        raw.parse().map_err(|err: T::Err| ConfigError::Invalid { name, reason: err.to_string() })
    })
}

pub fn parse_flag(lookup: Lookup<'_>, name: &'static str, default: bool) -> Result<bool, ConfigError> {
    let Some(raw) = value(lookup, name) else { return Ok(default) };
    match raw.to_ascii_lowercase().as_str() {
        "1" | "true" | "t" | "yes" | "y" | "on" => Ok(true),
        "0" | "false" | "f" | "no" | "n" | "off" => Ok(false),
        _ => Err(ConfigError::Invalid { name, reason: format!("{raw:?} is not a boolean") }),
    }
}

pub fn split_list(raw: &str) -> Vec<String> {
    raw.split(',').map(str::trim).filter(|item| !item.is_empty()).map(str::to_owned).collect()
}

pub fn sqlx_url(url: &str) -> String {
    url.strip_prefix("postgresql+asyncpg://").map_or_else(|| url.to_owned(), |rest| format!("postgresql://{rest}"))
}

pub fn swarm_database_url(lookup: Lookup<'_>, database: &str) -> Option<String> {
    let path = value(lookup, "DB_PASSWORD_FILE").unwrap_or_else(|| "/run/secrets/db_password".to_owned());
    let password = std::fs::read_to_string(path).ok()?;
    let password = percent_encoding::utf8_percent_encode(password.trim(), percent_encoding::NON_ALPHANUMERIC);
    Some(format!("postgresql://thecargo:{password}@pgbouncer:6432/{database}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Isoformat {
    pub at: DateTime<Utc>,
    pub naive: bool,
}

pub fn parse_isoformat(raw: &str) -> Option<Isoformat> {
    let mut text = raw.trim().to_owned();
    if text.len() > 10 && text.as_bytes()[10] == b' ' {
        text.replace_range(10..11, "T");
    }
    if let Some(stripped) = text.strip_suffix('Z') {
        text = format!("{stripped}+00:00");
    }
    if let Ok(aware) = DateTime::parse_from_rfc3339(&text) {
        return Some(Isoformat { at: aware.with_timezone(&Utc), naive: false });
    }
    for format in ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(&text, format) {
            return Some(Isoformat { at: naive.and_utc(), naive: true });
        }
    }
    None
}

pub fn instant(lookup: Lookup<'_>, name: &'static str) -> Result<Option<DateTime<Utc>>, ConfigError> {
    let Some(raw) = value(lookup, name) else { return Ok(None) };
    match parse_isoformat(&raw) {
        Some(moment) if !moment.naive => Ok(Some(moment.at)),
        _ => Err(ConfigError::Invalid { name, reason: format!("{raw:?} is not an instant with an offset") }),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
        move |key: &str| map.get(key).cloned()
    }

    #[test]
    fn every_missing_required_value_is_named() {
        let error = required([("A", Some("1".to_owned())), ("B", None), ("C", None)]).unwrap_err();
        assert_eq!(error.to_string(), "missing required configuration: B, C");
    }

    #[test]
    fn flags_take_the_spellings_pydantic_takes() {
        let lookup = env(&[("ON", "On"), ("OFF", "no"), ("BAD", "maybe")]);
        assert!(parse_flag(&lookup, "ON", false).unwrap());
        assert!(!parse_flag(&lookup, "OFF", true).unwrap());
        assert!(parse_flag(&lookup, "MISSING", true).unwrap());
        assert!(matches!(parse_flag(&lookup, "BAD", false), Err(ConfigError::Invalid { name: "BAD", .. })));
    }

    #[test]
    fn numbers_parse_or_stop_the_service() {
        let lookup = env(&[("PORT", " 8001 "), ("BURST", "lots")]);
        assert_eq!(parse_or(&lookup, "PORT", 8000_u16).unwrap(), 8001);
        assert_eq!(parse_or(&lookup, "ABSENT", 7_u32).unwrap(), 7);
        assert!(matches!(parse_or(&lookup, "BURST", 1_u32), Err(ConfigError::Invalid { name: "BURST", .. })));
    }

    #[test]
    fn the_asyncpg_url_becomes_a_sqlx_url() {
        assert_eq!(sqlx_url("postgresql+asyncpg://u:p@h:6432/db"), "postgresql://u:p@h:6432/db");
        assert_eq!(sqlx_url("postgresql://u:p@h/db"), "postgresql://u:p@h/db");
        assert_eq!(split_list(" a, b,,c "), ["a", "b", "c"]);
    }

    #[test]
    fn isoformat_reads_python_spellings() {
        let aware = parse_isoformat("2026-10-20 15:59:12.400000+00:00").unwrap();
        assert!(!aware.naive);
        assert_eq!(aware.at.to_rfc3339(), "2026-10-20T15:59:12.400+00:00");
        assert_eq!(
            parse_isoformat("2026-10-20T15:59:12Z").unwrap().at,
            aware.at - chrono::TimeDelta::microseconds(400_000)
        );
        assert!(parse_isoformat("2026-10-20T15:59").unwrap().naive);
        assert!(parse_isoformat("soon").is_none());
    }

    #[test]
    fn an_instant_setting_needs_an_offset() {
        let lookup = env(&[("FROM", "2026-10-20T00:00:00+00:00"), ("NAIVE", "2026-10-20T00:00:00")]);
        assert!(instant(&lookup, "FROM").unwrap().is_some());
        assert!(instant(&lookup, "ABSENT").unwrap().is_none());
        assert!(matches!(instant(&lookup, "NAIVE"), Err(ConfigError::Invalid { name: "NAIVE", .. })));
    }
}
