//! Secret data types for structured credential storage.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// The type of secret stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SecretType {
    Login,
    ApiKey,
    OAuthApp,
    SshKey,
    Note,
    Environment,
}

impl SecretType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Login => "login",
            Self::ApiKey => "api_key",
            Self::OAuthApp => "oauth_app",
            Self::SshKey => "ssh_key",
            Self::Note => "note",
            Self::Environment => "environment",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "login" => Some(Self::Login),
            "api_key" => Some(Self::ApiKey),
            "oauth_app" => Some(Self::OAuthApp),
            "ssh_key" => Some(Self::SshKey),
            "note" => Some(Self::Note),
            "environment" => Some(Self::Environment),
            _ => None,
        }
    }
}

impl std::fmt::Display for SecretType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Structured secret data with typed variants.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SecretData {
    Login {
        username: String,
        password: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        url: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        notes: Option<String>,
    },
    ApiKey {
        key: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        endpoint: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        notes: Option<String>,
    },
    OAuthApp {
        client_id: String,
        client_secret: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        redirect_uri: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        scopes: Option<Vec<String>>,
    },
    SshKey {
        private_key: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        public_key: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        passphrase: Option<String>,
    },
    Note {
        content: String,
    },
    Environment {
        variables: HashMap<String, String>,
    },
}

impl SecretData {
    /// Get the type of this secret.
    pub fn secret_type(&self) -> SecretType {
        match self {
            Self::Login { .. } => SecretType::Login,
            Self::ApiKey { .. } => SecretType::ApiKey,
            Self::OAuthApp { .. } => SecretType::OAuthApp,
            Self::SshKey { .. } => SecretType::SshKey,
            Self::Note { .. } => SecretType::Note,
            Self::Environment { .. } => SecretType::Environment,
        }
    }

    /// Extract the primary secret value for substitution.
    ///
    /// Returns the most commonly needed value for each type:
    /// - Login: password
    /// - ApiKey: key
    /// - OAuthApp: client_secret
    /// - SshKey: private_key
    /// - Note: content
    /// - Environment: JSON of all variables
    pub fn primary_value(&self) -> String {
        match self {
            Self::Login { password, .. } => password.clone(),
            Self::ApiKey { key, .. } => key.clone(),
            Self::OAuthApp { client_secret, .. } => client_secret.clone(),
            Self::SshKey { private_key, .. } => private_key.clone(),
            Self::Note { content } => content.clone(),
            Self::Environment { variables } => {
                serde_json::to_string(variables).unwrap_or_default()
            }
        }
    }

    /// Get a specific field from the secret.
    pub fn get_field(&self, field: &str) -> Option<String> {
        match self {
            Self::Login {
                username,
                password,
                url,
                notes,
            } => match field {
                "username" => Some(username.clone()),
                "password" => Some(password.clone()),
                "url" => url.clone(),
                "notes" => notes.clone(),
                _ => None,
            },
            Self::ApiKey {
                key,
                endpoint,
                notes,
            } => match field {
                "key" => Some(key.clone()),
                "endpoint" => endpoint.clone(),
                "notes" => notes.clone(),
                _ => None,
            },
            Self::OAuthApp {
                client_id,
                client_secret,
                redirect_uri,
                scopes,
            } => match field {
                "client_id" => Some(client_id.clone()),
                "client_secret" => Some(client_secret.clone()),
                "redirect_uri" => redirect_uri.clone(),
                "scopes" => scopes.as_ref().map(|s| s.join(",")),
                _ => None,
            },
            Self::SshKey {
                private_key,
                public_key,
                passphrase,
            } => match field {
                "private_key" => Some(private_key.clone()),
                "public_key" => public_key.clone(),
                "passphrase" => passphrase.clone(),
                _ => None,
            },
            Self::Note { content } => match field {
                "content" => Some(content.clone()),
                _ => None,
            },
            Self::Environment { variables } => variables.get(field).cloned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_type_roundtrip() {
        for st in [
            SecretType::Login,
            SecretType::ApiKey,
            SecretType::OAuthApp,
            SecretType::SshKey,
            SecretType::Note,
            SecretType::Environment,
        ] {
            assert_eq!(SecretType::from_str(st.as_str()), Some(st));
        }
    }

    #[test]
    fn secret_data_serialization() {
        let data = SecretData::Login {
            username: "user".into(),
            password: "pass".into(),
            url: Some("https://example.com".into()),
            notes: None,
        };

        let json = serde_json::to_string(&data).unwrap();
        assert!(json.contains("\"type\":\"login\""));
        assert!(json.contains("\"username\":\"user\""));
        assert!(!json.contains("notes")); // skip_serializing_if = None

        let restored: SecretData = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.secret_type(), SecretType::Login);
    }

    #[test]
    fn primary_value_extraction() {
        let login = SecretData::Login {
            username: "user".into(),
            password: "secret123".into(),
            url: None,
            notes: None,
        };
        assert_eq!(login.primary_value(), "secret123");

        let api = SecretData::ApiKey {
            key: "api-key-value".into(),
            endpoint: None,
            notes: None,
        };
        assert_eq!(api.primary_value(), "api-key-value");
    }

    #[test]
    fn field_access() {
        let login = SecretData::Login {
            username: "admin".into(),
            password: "hunter2".into(),
            url: Some("https://example.com".into()),
            notes: None,
        };
        assert_eq!(login.get_field("username"), Some("admin".into()));
        assert_eq!(login.get_field("password"), Some("hunter2".into()));
        assert_eq!(
            login.get_field("url"),
            Some("https://example.com".into())
        );
        assert_eq!(login.get_field("notes"), None);
        assert_eq!(login.get_field("nonexistent"), None);
    }

    #[test]
    fn environment_variables() {
        let env = SecretData::Environment {
            variables: HashMap::from([
                ("DB_HOST".into(), "localhost".into()),
                ("DB_PORT".into(), "5432".into()),
            ]),
        };
        assert_eq!(env.get_field("DB_HOST"), Some("localhost".into()));
        assert_eq!(env.get_field("DB_PORT"), Some("5432".into()));
        assert_eq!(env.get_field("DB_MISSING"), None);
    }
}
