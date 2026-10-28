use std::time::Duration;

use axum::Router;
use redis::aio::{ConnectionManager, ConnectionManagerConfig};
use tokio::net::TcpListener;
use tokio::task::{JoinError, JoinHandle};
use tokio_util::sync::CancellationToken;

pub async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(err) = tokio::signal::ctrl_c().await {
            tracing::error!(error = %err, "cannot listen for ctrl-c");
            std::future::pending::<()>().await;
        }
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(err) => {
                tracing::error!(error = %err, "cannot listen for SIGTERM");
                std::future::pending::<()>().await;
            }
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
}

pub async fn connect_redis(url: &str) -> Result<ConnectionManager, redis::RedisError> {
    let client = redis::Client::open(url)?;
    let settings = ConnectionManagerConfig::new()
        .set_connection_timeout(Some(Duration::from_secs(3)))
        .set_response_timeout(Some(Duration::from_secs(2)));
    ConnectionManager::new_with_config(client, settings).await
}

pub async fn serve_until(listener: TcpListener, app: Router, cancel: CancellationToken) -> std::io::Result<()> {
    axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>())
        .with_graceful_shutdown(cancel.cancelled_owned())
        .await
}

pub async fn join_with_grace<T>(handle: JoinHandle<T>, grace: Duration) -> Option<Result<T, JoinError>> {
    tokio::time::timeout(grace, handle).await.ok()
}
