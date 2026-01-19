//! JWT Token Validation
//!
//! Validates tokens and extracts claims. Does NOT issue tokens -
//! that's the job of your OAuth provider (Auth0, Cognito, etc).

use jsonwebtoken::{Algorithm, DecodingKey, TokenData, Validation, decode};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum JwtError {
    #[error("Missing Authorization header")]
    MissingHeader,

    #[error("Invalid Authorization header format (expected 'Bearer <token>')")]
    InvalidFormat,

    #[error("Token validation failed: {0}")]
    ValidationFailed(#[from] jsonwebtoken::errors::Error),

    #[error("Token expired")]
    Expired,

    #[error("Invalid issuer")]
    InvalidIssuer,

    #[error("Invalid audience")]
    InvalidAudience,
}

/// Standard JWT claims plus custom fields for multi-tenancy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Claims {
    /// Subject (user ID)
    pub sub: String,

    /// Expiration time (Unix timestamp)
    pub exp: u64,

    /// Issued at (Unix timestamp)
    #[serde(default)]
    pub iat: u64,

    /// Issuer
    #[serde(default)]
    pub iss: Option<String>,

    /// Audience
    #[serde(default)]
    pub aud: Option<String>,

    /// Tenant ID for multi-tenancy (custom claim)
    #[serde(default)]
    pub tenant_id: Option<String>,

    /// Scopes/permissions (custom claim)
    #[serde(default)]
    pub scope: Option<String>,
}

impl Claims {
    /// Get the user ID (subject)
    pub fn user_id(&self) -> &str {
        &self.sub
    }

    /// Get tenant ID, falling back to user ID if not set
    pub fn tenant_id(&self) -> &str {
        self.tenant_id.as_deref().unwrap_or(&self.sub)
    }

    /// Check if a scope is present
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scope
            .as_ref()
            .map(|s| s.split_whitespace().any(|s| s == scope))
            .unwrap_or(false)
    }
}

/// JWT Validator configuration
pub struct JwtValidator {
    decoding_key: DecodingKey,
    validation: Validation,
}

impl JwtValidator {
    /// Create a validator for HS256 (symmetric) tokens
    ///
    /// Use this for development/testing. In production, prefer RS256.
    pub fn hs256(secret: &[u8]) -> Self {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.validate_exp = true;

        Self {
            decoding_key: DecodingKey::from_secret(secret),
            validation,
        }
    }

    /// Create a validator for RS256 (asymmetric) tokens
    ///
    /// Use this in production with your OAuth provider's public key.
    pub fn rs256_pem(public_key_pem: &[u8]) -> Result<Self, JwtError> {
        let mut validation = Validation::new(Algorithm::RS256);
        validation.validate_exp = true;

        Ok(Self {
            decoding_key: DecodingKey::from_rsa_pem(public_key_pem)?,
            validation,
        })
    }

    /// Create a validator for RS256 using JWKS components (n, e)
    pub fn rs256_components(n: &str, e: &str) -> Result<Self, JwtError> {
        let mut validation = Validation::new(Algorithm::RS256);
        validation.validate_exp = true;

        Ok(Self {
            decoding_key: DecodingKey::from_rsa_components(n, e)?,
            validation,
        })
    }

    /// Require a specific issuer
    pub fn with_issuer(mut self, issuer: &str) -> Self {
        self.validation.set_issuer(&[issuer]);
        self
    }

    /// Require a specific audience
    pub fn with_audience(mut self, audience: &str) -> Self {
        self.validation.set_audience(&[audience]);
        self
    }

    /// Extract token from Authorization header
    pub fn extract_token(auth_header: &str) -> Result<&str, JwtError> {
        auth_header
            .strip_prefix("Bearer ")
            .ok_or(JwtError::InvalidFormat)
    }

    /// Validate a token and return claims
    pub fn validate(&self, token: &str) -> Result<Claims, JwtError> {
        let token_data: TokenData<Claims> = decode(token, &self.decoding_key, &self.validation)?;
        Ok(token_data.claims)
    }

    /// Validate from Authorization header value
    pub fn validate_header(&self, auth_header: &str) -> Result<Claims, JwtError> {
        let token = Self::extract_token(auth_header)?;
        self.validate(token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header, encode};

    fn create_test_token(claims: &Claims, secret: &[u8]) -> String {
        encode(
            &Header::new(Algorithm::HS256),
            claims,
            &EncodingKey::from_secret(secret),
        )
        .unwrap()
    }

    #[test]
    fn test_validate_valid_token() {
        let secret = b"super-secret-key-for-testing";
        let validator = JwtValidator::hs256(secret);

        let claims = Claims {
            sub: "user-123".to_string(),
            exp: (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs())
                + 3600, // 1 hour from now
            iat: 0,
            iss: None,
            aud: None,
            tenant_id: Some("tenant-456".to_string()),
            scope: Some("read write".to_string()),
        };

        let token = create_test_token(&claims, secret);
        let validated = validator.validate(&token).unwrap();

        assert_eq!(validated.user_id(), "user-123");
        assert_eq!(validated.tenant_id(), "tenant-456");
        assert!(validated.has_scope("read"));
        assert!(validated.has_scope("write"));
        assert!(!validated.has_scope("admin"));
    }

    #[test]
    fn test_validate_expired_token() {
        let secret = b"super-secret-key-for-testing";
        let validator = JwtValidator::hs256(secret);

        let claims = Claims {
            sub: "user-123".to_string(),
            exp: 1000, // Way in the past
            iat: 0,
            iss: None,
            aud: None,
            tenant_id: None,
            scope: None,
        };

        let token = create_test_token(&claims, secret);
        let result = validator.validate(&token);

        assert!(result.is_err());
    }

    #[test]
    fn test_extract_token() {
        let header = "Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.test";
        let token = JwtValidator::extract_token(header).unwrap();
        assert_eq!(token, "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.test");
    }

    #[test]
    fn test_extract_token_invalid_format() {
        let header = "Basic abc123";
        let result = JwtValidator::extract_token(header);
        assert!(result.is_err());
    }

    #[test]
    fn test_tenant_id_fallback() {
        let claims = Claims {
            sub: "user-123".to_string(),
            exp: 0,
            iat: 0,
            iss: None,
            aud: None,
            tenant_id: None, // No tenant_id
            scope: None,
        };

        // Should fall back to user_id
        assert_eq!(claims.tenant_id(), "user-123");
    }
}
