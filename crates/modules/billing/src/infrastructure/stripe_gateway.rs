//! [`PaymentGateway`] over `warptalk-stripe`.

use async_trait::async_trait;
use warptalk_kernel::WorkspaceId;
use warptalk_stripe::{CheckoutRequest, CheckoutSession, StripeClient, StripeError, Subscription};

use crate::application::ports::PaymentGateway;

pub struct StripeGateway {
    client: StripeClient,
}

impl StripeGateway {
    pub fn new(client: StripeClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl PaymentGateway for StripeGateway {
    async fn create_customer(
        &self,
        ws: WorkspaceId,
        idempotency_key: &str,
    ) -> Result<String, StripeError> {
        self.client
            .create_customer(
                None,
                None,
                &[("workspace_id".to_owned(), ws.to_string())],
                Some(idempotency_key),
            )
            .await
            .map(|c| c.id)
    }

    async fn create_checkout(
        &self,
        req: &CheckoutRequest,
        idempotency_key: &str,
    ) -> Result<CheckoutSession, StripeError> {
        self.client
            .create_checkout_session(req, Some(idempotency_key))
            .await
    }

    async fn create_portal(&self, customer: &str, return_url: &str) -> Result<String, StripeError> {
        self.client
            .create_portal_session(customer, return_url)
            .await
            .map(|s| s.url)
    }

    async fn subscription(&self, id: &str) -> Result<Subscription, StripeError> {
        self.client.retrieve_subscription(id).await
    }
}
