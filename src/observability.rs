use secrecy::SecretString;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, fmt};

#[derive(Debug, Clone)]
pub struct SentrySettings {
    pub dsn: Option<SecretString>,
    pub environment: String,
    pub server_name: String,
    pub release: Option<String>,
}

pub fn init_logging(log_level: &str) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(log_level.to_ascii_lowercase()));
    #[cfg(feature = "sentry")]
    let error_layer = Some(sentry::integrations::tracing::layer());
    #[cfg(not(feature = "sentry"))]
    let error_layer: Option<tracing_subscriber::layer::Identity> = None;
    let result = tracing_subscriber::registry()
        .with(filter)
        .with(error_layer)
        .with(fmt::layer().json().with_target(false).with_current_span(true))
        .try_init();
    if let Err(err) = result {
        eprintln!("logging was already initialised: {err}");
    }
}

// Errors go to GlitchTip through its Sentry endpoint; no SENTRY_DSN means no client at all.
#[cfg(feature = "sentry")]
pub fn init_sentry(settings: &SentrySettings) -> Option<sentry::ClientInitGuard> {
    use secrecy::ExposeSecret;

    let dsn = settings.dsn.as_ref().map(|dsn| dsn.expose_secret().trim().to_owned()).filter(|dsn| !dsn.is_empty())?;
    install_tls_provider();
    let mut options = sentry::ClientOptions::default();
    options.environment = Some(settings.environment.clone().into());
    options.server_name = Some(settings.server_name.clone().into());
    options.release = settings.release.clone().map(Into::into);
    options.send_default_pii = false;
    let guard = sentry::init((dsn, options));
    sentry::configure_scope(|scope| scope.set_tag("service", &settings.server_name));
    tracing::info!(service = %settings.server_name, env = %settings.environment, "sentry error tracking initialised");
    Some(guard)
}

// reqwest is built without a default rustls provider (sentry's transport asks for that), so one is
// installed before any HTTP client exists. Repeated calls are harmless.
pub fn install_tls_provider() {
    if rustls::crypto::ring::default_provider().install_default().is_err() {
        tracing::trace!("a rustls crypto provider was already installed");
    }
}
