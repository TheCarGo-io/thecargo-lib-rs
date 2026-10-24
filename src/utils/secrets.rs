use std::path::Path;

pub fn read_secret(lookup: &impl Fn(&str) -> Option<String>, key: &str) -> Option<String> {
    if let Some(path) = lookup(&format!("{key}_FILE"))
        && Path::new(&path).is_file()
    {
        return std::fs::read_to_string(path).ok().map(|raw| raw.trim().to_owned());
    }
    lookup(key)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
        move |key: &str| map.get(key).cloned()
    }

    #[test]
    fn a_secret_file_wins_over_the_variable() {
        let dir = std::env::temp_dir().join(format!("tc-secret-{}", uuid::Uuid::new_v4()));
        std::fs::write(&dir, "from-file\n").unwrap();
        let path = dir.to_string_lossy().into_owned();

        let pairs = [("JWT_SECRET_KEY_FILE", path.as_str()), ("JWT_SECRET_KEY", "from-env")];
        let env = lookup(&pairs);
        assert_eq!(read_secret(&env, "JWT_SECRET_KEY").as_deref(), Some("from-file"));

        std::fs::remove_file(dir).unwrap();
    }

    #[test]
    fn a_missing_file_falls_back_to_the_variable() {
        let env = lookup(&[("JWT_SECRET_KEY_FILE", "/nonexistent/secret"), ("JWT_SECRET_KEY", "from-env")]);
        assert_eq!(read_secret(&env, "JWT_SECRET_KEY").as_deref(), Some("from-env"));
        assert_eq!(read_secret(&lookup(&[]), "JWT_SECRET_KEY"), None);
    }
}
