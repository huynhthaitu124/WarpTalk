//! Stripe: checkout sessions, customers, billing portal, subscription retrieval and webhook
//! signature verification. Hand-rolled over `reqwest` (form-encoded requests, JSON responses).
//!
//! Anything that could not be checked against the live API from the development environment is
//! marked `// UNVERIFIED`. The secret key and the webhook secret are never logged (`Debug` of the
//! config redacts them).

mod client;
mod error;
mod model;
pub mod webhook;

pub use client::{StripeClient, StripeConfig};
pub use error::StripeError;
pub use model::{
    BillingPortalSession, CheckoutLine, CheckoutMode, CheckoutRequest, CheckoutSession, Customer,
    Subscription,
};
pub use webhook::{Event, SignatureError, parse_event, verify_signature};
