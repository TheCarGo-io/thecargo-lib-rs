use percent_encoding::percent_decode_str;
use url::Url;

fn decode(raw: &str) -> String {
    percent_decode_str(raw).decode_utf8_lossy().into_owned()
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DsnError {
    #[error("the ClickHouse DSN is not a URL")]
    NotUrl(#[from] url::ParseError),
    #[error("the ClickHouse DSN scheme {0:?} is not supported")]
    Scheme(String),
    #[error("the ClickHouse DSN has no host")]
    NoHost,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dsn {
    pub url: String,
    pub user: Option<String>,
    pub password: Option<String>,
    pub database: Option<String>,
}

// The DSN clickhouse_connect takes: clickhouse://user:password@host:8123/database over HTTP.
pub fn parse_dsn(raw: &str) -> Result<Dsn, DsnError> {
    let parsed = Url::parse(raw.trim())?;
    let scheme = match parsed.scheme() {
        "clickhouse" | "clickhouse+http" | "http" => "http",
        "clickhouses" | "clickhouse+https" | "https" => "https",
        other => return Err(DsnError::Scheme(other.to_owned())),
    };
    let host = parsed.host_str().filter(|host| !host.is_empty()).ok_or(DsnError::NoHost)?;
    let port = parsed.port().unwrap_or(if scheme == "https" { 8443 } else { 8123 });
    let user = Some(decode(parsed.username())).filter(|user| !user.is_empty());
    let password = parsed.password().map(decode);
    let database = Some(parsed.path().trim_matches('/').to_owned()).filter(|db| !db.is_empty());
    Ok(Dsn { url: format!("{scheme}://{host}:{port}"), user, password, database })
}

pub fn client(dsn: &Dsn) -> clickhouse::Client {
    let mut client = clickhouse::Client::default().with_url(&dsn.url);
    if let Some(user) = &dsn.user {
        client = client.with_user(user);
    }
    if let Some(password) = &dsn.password {
        client = client.with_password(password);
    }
    if let Some(database) = &dsn.database {
        client = client.with_database(database);
    }
    client
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dev_dsn_maps_to_an_http_client() {
        let dsn = parse_dsn("clickhouse://analytics:p%40ss+word@thecargo_clickhouse:8123/thecargo").unwrap();
        assert_eq!(dsn.url, "http://thecargo_clickhouse:8123");
        assert_eq!(dsn.user.as_deref(), Some("analytics"));
        assert_eq!(dsn.password.as_deref(), Some("p@ss+word"));
        assert_eq!(dsn.database.as_deref(), Some("thecargo"));
    }

    #[test]
    fn the_port_and_database_are_optional() {
        let dsn = parse_dsn("clickhouse://localhost").unwrap();
        assert_eq!(dsn.url, "http://localhost:8123");
        assert_eq!((dsn.user, dsn.password, dsn.database), (None, None, None));
    }

    #[test]
    fn other_schemes_are_refused() {
        assert_eq!(parse_dsn("postgres://x@y/z"), Err(DsnError::Scheme("postgres".to_owned())));
        assert!(matches!(parse_dsn("not a url"), Err(DsnError::NotUrl(_))));
    }
}
