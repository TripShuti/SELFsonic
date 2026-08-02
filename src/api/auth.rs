//! Формування параметрів авторизації Subsonic.
//!
//! Password-based auth (AGENT.md): `u`, `p`, `v`, `c`, без token/salt.

/// Загальні параметри кожного запиту.
pub fn base_params(username: &str, password: &str, api_version: &str, client: &str) -> Vec<(&'static str, String)> {
    vec![
        ("u", username.to_string()),
        ("p", password.to_string()),
        ("v", api_version.to_string()),
        ("c", client.to_string()),
        ("f", "json".to_string()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_params_contains_expected_keys() {
        let params = base_params("alice", "s3cret", "1.16.1", "SELFsonic");
        let map: std::collections::HashMap<_, _> = params.into_iter().collect();
        assert_eq!(map.get("u").unwrap(), "alice");
        assert_eq!(map.get("p").unwrap(), "s3cret");
        assert_eq!(map.get("v").unwrap(), "1.16.1");
        assert_eq!(map.get("c").unwrap(), "SELFsonic");
        assert_eq!(map.get("f").unwrap(), "json");
    }
}
