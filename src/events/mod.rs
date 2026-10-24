pub mod consumer;

// aio-pika reads "amqp://host:5672/" as the default vhost "/"; lapin reads the empty path segment
// as a vhost named "" and the broker refuses it. The Python services' URLs end in "/", so
// translate them the way aio-pika does.
pub fn amqp_url(raw: &str) -> String {
    match url::Url::parse(raw) {
        Ok(mut parsed) if matches!(parsed.path(), "" | "/") => {
            parsed.set_path("/%2f");
            parsed.to_string()
        }
        _ => raw.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::amqp_url;

    #[test]
    fn a_trailing_slash_means_the_default_vhost() {
        assert_eq!(amqp_url("amqp://guest:pw@localhost:5672/"), "amqp://guest:pw@localhost:5672/%2f");
        assert_eq!(amqp_url("amqp://guest:pw@localhost:5672"), "amqp://guest:pw@localhost:5672/%2f");
        assert_eq!(amqp_url("amqp://x:pw@localhost:5672/x"), "amqp://x:pw@localhost:5672/x");
        assert_eq!(amqp_url("amqp://h/%2f"), "amqp://h/%2f");
    }
}
