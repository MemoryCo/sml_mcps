//! JWT Authentication
//!
//! Validates JWT tokens and extracts claims for multi-tenancy.

mod jwt;

pub use jwt::{JwtValidator, Claims, JwtError};
