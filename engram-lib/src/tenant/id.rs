//! Tenant ID generation from user IDs.
//!
//! User IDs can contain arbitrary characters. Tenant IDs must be safe for:
//! - Filesystem paths (no slashes, no path traversal)
//! - Directory names (reasonable length, ASCII-safe)
//!
//! Rules:
//! - If user_id is alphanumeric + dash + underscore and <= 64 chars, use it directly
//! - Otherwise, hash to `t_<sha256_prefix>`

use sha2::{Digest, Sha256};

/// Maximum length for a direct tenant ID (no hashing).
const MAX_DIRECT_LENGTH: usize = 64;

/// Length of the SHA256 prefix used for hashed tenant IDs.
const HASH_PREFIX_LENGTH: usize = 16;

/// Convert a user ID to a safe tenant ID.
///
/// If the user ID contains only safe characters (alphanumeric, dash, underscore)
/// and is within the length limit, it's used directly. Otherwise, it's hashed.
///
/// # Examples
///
/// ```
/// use engram_lib::tenant::tenant_id_from_user;
///
/// // Safe user IDs pass through
/// assert_eq!(tenant_id_from_user("alice"), "alice");
/// assert_eq!(tenant_id_from_user("user-123"), "user-123");
/// assert_eq!(tenant_id_from_user("user_name"), "user_name");
///
/// // Unsafe user IDs get hashed
/// assert!(tenant_id_from_user("../etc/passwd").starts_with("t_"));
/// assert!(tenant_id_from_user("user@example.com").starts_with("t_"));
/// ```
pub fn tenant_id_from_user(user_id: &str) -> String {
    if is_safe_tenant_id(user_id) {
        user_id.to_string()
    } else {
        hash_to_tenant_id(user_id)
    }
}

/// Check if a user ID can be used directly as a tenant ID.
fn is_safe_tenant_id(user_id: &str) -> bool {
    // Must not be empty
    if user_id.is_empty() {
        return false;
    }

    // Must be within length limit
    if user_id.len() > MAX_DIRECT_LENGTH {
        return false;
    }

    // Must contain only safe characters
    user_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Hash a user ID to a tenant ID.
fn hash_to_tenant_id(user_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(user_id.as_bytes());
    let hash = hasher.finalize();

    // Take first N bytes and encode as hex
    let hex: String = hash[..HASH_PREFIX_LENGTH / 2]
        .iter()
        .flat_map(|b| {
            let [h, l] = hex_encode_byte(*b);
            [h, l]
        })
        .collect();

    format!("t_{}", hex)
}

fn hex_encode_byte(b: u8) -> [char; 2] {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    [
        HEX[(b >> 4) as usize] as char,
        HEX[(b & 0xf) as usize] as char,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_user_ids_pass_through() {
        assert_eq!(tenant_id_from_user("alice"), "alice");
        assert_eq!(tenant_id_from_user("bob123"), "bob123");
        assert_eq!(tenant_id_from_user("user-name"), "user-name");
        assert_eq!(tenant_id_from_user("user_name"), "user_name");
        assert_eq!(tenant_id_from_user("User-Name_123"), "User-Name_123");
    }

    #[test]
    fn path_traversal_gets_hashed() {
        let id = tenant_id_from_user("../etc/passwd");
        assert!(id.starts_with("t_"));
        assert!(!id.contains('/'));
        assert!(!id.contains('.'));
    }

    #[test]
    fn slashes_get_hashed() {
        let id = tenant_id_from_user("user/subdir");
        assert!(id.starts_with("t_"));
        assert!(!id.contains('/'));
    }

    #[test]
    fn backslashes_get_hashed() {
        let id = tenant_id_from_user("user\\subdir");
        assert!(id.starts_with("t_"));
        assert!(!id.contains('\\'));
    }

    #[test]
    fn email_addresses_get_hashed() {
        let id = tenant_id_from_user("user@example.com");
        assert!(id.starts_with("t_"));
        assert!(!id.contains('@'));
        assert!(!id.contains('.'));
    }

    #[test]
    fn unicode_gets_hashed() {
        // Cafe with accent: café
        let id = tenant_id_from_user("user_caf\u{00E9}");
        assert!(id.starts_with("t_"));
    }

    #[test]
    fn emoji_gets_hashed() {
        // Fire emoji
        let id = tenant_id_from_user("user_\u{1F525}");
        assert!(id.starts_with("t_"));
    }

    #[test]
    fn long_ids_get_hashed() {
        let long_id = "a".repeat(65);
        let id = tenant_id_from_user(&long_id);
        assert!(id.starts_with("t_"));
        assert!(id.len() < 65);
    }

    #[test]
    fn max_length_passes_through() {
        let max_id = "a".repeat(64);
        assert_eq!(tenant_id_from_user(&max_id), max_id);
    }

    #[test]
    fn empty_string_gets_hashed() {
        let id = tenant_id_from_user("");
        assert!(id.starts_with("t_"));
    }

    #[test]
    fn null_bytes_get_hashed() {
        let id = tenant_id_from_user("user\0name");
        assert!(id.starts_with("t_"));
        assert!(!id.contains('\0'));
    }

    #[test]
    fn same_input_same_output() {
        let id1 = tenant_id_from_user("user@example.com");
        let id2 = tenant_id_from_user("user@example.com");
        assert_eq!(id1, id2);
    }

    #[test]
    fn different_input_different_output() {
        let id1 = tenant_id_from_user("user1@example.com");
        let id2 = tenant_id_from_user("user2@example.com");
        assert_ne!(id1, id2);
    }

    #[test]
    fn hashed_ids_are_valid_filenames() {
        let id = tenant_id_from_user("user@example.com/../../../etc/passwd");
        // Should only contain alphanumeric and underscore
        assert!(id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
    }
}
