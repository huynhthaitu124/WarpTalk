//! Entities, value objects, state machines. Pure Rust, no IO.

pub mod account;
pub mod error;
pub mod password;
pub mod policy;
pub mod validation;

pub use account::{AccountFlags, AccountStatus};
pub use error::IdentityError;
pub use policy::Policy;
pub mod profile;
pub use profile::{Profile, Settings, UserSummary};
