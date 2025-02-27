use api_models::payments::PaymentMethodData;
use common_utils::pii;
use error_stack::{IntoReport, ResultExt};
use masking::{ExposeInterface, Secret};
use serde::{Deserialize, Serialize};

use crate::{
    connector::utils::{
        missing_field_err, AddressDetailsData, CardData, PaymentsAuthorizeRequestData,
        PaymentsSyncRequestData, RouterData,
    },
    core::errors,
    types::{self, api, storage::enums, MandateReference},
};

#[derive(Debug, Serialize)]
pub struct PayRequest {
    buyer_name: Secret<String>,
    buyer_email: pii::Email,
    payme_sale_id: String,
    #[serde(flatten)]
    card: PaymeCard,
}

#[derive(Debug, Serialize)]
pub struct MandateRequest {
    currency: enums::Currency,
    sale_price: i64,
    transaction_id: String,
    product_name: String,
    sale_return_url: String,
    seller_payme_id: Secret<String>,
    sale_callback_url: String,
    buyer_key: Secret<String>,
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum PaymePaymentRequest {
    MandateRequest(MandateRequest),
    PayRequest(PayRequest),
}

#[derive(Debug, Serialize)]
pub struct PaymeQuerySaleRequest {
    sale_payme_id: String,
    seller_payme_id: Secret<String>,
}

#[derive(Debug, Serialize)]
pub struct PaymeQueryTransactionRequest {
    payme_transaction_id: String,
    seller_payme_id: Secret<String>,
}

#[derive(Debug, Serialize)]
pub struct PaymeCard {
    credit_card_cvv: Secret<String>,
    credit_card_exp: Secret<String>,
    credit_card_number: cards::CardNumber,
}

#[derive(Debug, Serialize)]
pub struct GenerateSaleRequest {
    currency: enums::Currency,
    sale_type: SaleType,
    sale_price: i64,
    transaction_id: String,
    product_name: String,
    sale_return_url: String,
    seller_payme_id: Secret<String>,
    sale_callback_url: String,
    sale_payment_method: SalePaymentMethod,
}

#[derive(Debug, Deserialize)]
pub struct GenerateSaleResponse {
    payme_sale_id: String,
}

impl<F, T>
    TryFrom<types::ResponseRouterData<F, PaymePaymentsResponse, T, types::PaymentsResponseData>>
    for types::RouterData<F, T, types::PaymentsResponseData>
{
    type Error = error_stack::Report<errors::ConnectorError>;
    fn try_from(
        item: types::ResponseRouterData<F, PaymePaymentsResponse, T, types::PaymentsResponseData>,
    ) -> Result<Self, Self::Error> {
        match item.response {
            // To handle webhook response
            PaymePaymentsResponse::PaymePaySaleResponse(response) => {
                Self::try_from(types::ResponseRouterData {
                    response,
                    data: item.data,
                    http_code: item.http_code,
                })
            }
            // To handle PSync response
            PaymePaymentsResponse::SaleQueryResponse(response) => {
                Self::try_from(types::ResponseRouterData {
                    response,
                    data: item.data,
                    http_code: item.http_code,
                })
            }
        }
    }
}

impl<F, T>
    TryFrom<types::ResponseRouterData<F, PaymePaySaleResponse, T, types::PaymentsResponseData>>
    for types::RouterData<F, T, types::PaymentsResponseData>
{
    type Error = error_stack::Report<errors::ConnectorError>;
    fn try_from(
        item: types::ResponseRouterData<F, PaymePaySaleResponse, T, types::PaymentsResponseData>,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            status: enums::AttemptStatus::from(item.response.sale_status),
            response: Ok(types::PaymentsResponseData::TransactionResponse {
                resource_id: types::ResponseId::ConnectorTransactionId(item.response.payme_sale_id),
                redirection_data: None,
                mandate_reference: item.response.buyer_key.map(|buyer_key| MandateReference {
                    connector_mandate_id: Some(buyer_key.expose()),
                    payment_method_id: None,
                }),
                connector_metadata: Some(
                    serde_json::to_value(PaymeMetadata {
                        payme_transaction_id: item.response.payme_transaction_id,
                    })
                    .into_report()
                    .change_context(errors::ConnectorError::ResponseHandlingFailed)?,
                ),
                network_txn_id: None,
                connector_response_reference_id: None,
            }),
            ..item.data
        })
    }
}

impl<F, T> TryFrom<types::ResponseRouterData<F, SaleQueryResponse, T, types::PaymentsResponseData>>
    for types::RouterData<F, T, types::PaymentsResponseData>
{
    type Error = error_stack::Report<errors::ConnectorError>;
    fn try_from(
        item: types::ResponseRouterData<F, SaleQueryResponse, T, types::PaymentsResponseData>,
    ) -> Result<Self, Self::Error> {
        // Only one element would be present since we are passing one transaction id in the PSync request
        let transaction_response = item
            .response
            .items
            .first()
            .cloned()
            .ok_or(errors::ConnectorError::ResponseHandlingFailed)?;
        Ok(Self {
            status: enums::AttemptStatus::from(transaction_response.sale_status),
            response: Ok(types::PaymentsResponseData::TransactionResponse {
                resource_id: types::ResponseId::ConnectorTransactionId(
                    transaction_response.sale_payme_id,
                ),
                redirection_data: None,
                // mandate reference will be updated with webhooks only. That has been handled with PaymePaySaleResponse struct
                mandate_reference: None,
                connector_metadata: None,
                network_txn_id: None,
                connector_response_reference_id: None,
            }),
            ..item.data
        })
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SaleType {
    Sale,
    Authorize,
    Token,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SalePaymentMethod {
    CreditCard,
}

impl TryFrom<&types::PaymentsInitRouterData> for GenerateSaleRequest {
    type Error = error_stack::Report<errors::ConnectorError>;
    fn try_from(item: &types::PaymentsInitRouterData) -> Result<Self, Self::Error> {
        let sale_type = SaleType::try_from(item)?;
        let seller_payme_id = PaymeAuthType::try_from(&item.connector_auth_type)?.seller_payme_id;
        let order_details = item.request.get_order_details()?;
        let product_name = order_details
            .first()
            .ok_or_else(missing_field_err("order_details"))?
            .product_name
            .clone();
        Ok(Self {
            currency: item.request.currency,
            sale_type,
            sale_price: item.request.amount,
            transaction_id: item.payment_id.clone(),
            product_name,
            sale_return_url: item.request.get_return_url()?,
            seller_payme_id,
            sale_callback_url: item.request.get_webhook_url()?,
            sale_payment_method: SalePaymentMethod::try_from(&item.request.payment_method_data)?,
        })
    }
}

impl TryFrom<&types::PaymentsInitRouterData> for SaleType {
    type Error = error_stack::Report<errors::ConnectorError>;
    fn try_from(value: &types::PaymentsInitRouterData) -> Result<Self, Self::Error> {
        let sale_type = if value.request.setup_mandate_details.is_some() {
            // First mandate
            Self::Token
        } else {
            // Normal payments
            match value.request.is_auto_capture()? {
                true => Self::Sale,
                false => Self::Authorize,
            }
        };
        Ok(sale_type)
    }
}

impl TryFrom<&PaymentMethodData> for SalePaymentMethod {
    type Error = error_stack::Report<errors::ConnectorError>;
    fn try_from(item: &PaymentMethodData) -> Result<Self, Self::Error> {
        match item {
            PaymentMethodData::Card(_) => Ok(Self::CreditCard),
            PaymentMethodData::Wallet(_)
            | PaymentMethodData::PayLater(_)
            | PaymentMethodData::BankRedirect(_)
            | PaymentMethodData::BankDebit(_)
            | PaymentMethodData::BankTransfer(_)
            | PaymentMethodData::Crypto(_)
            | PaymentMethodData::MandatePayment
            | PaymentMethodData::Reward(_)
            | PaymentMethodData::GiftCard(_)
            | PaymentMethodData::CardRedirect(_)
            | PaymentMethodData::Upi(_)
            | api::PaymentMethodData::Voucher(_) => {
                Err(errors::ConnectorError::NotImplemented("Payment methods".to_string()).into())
            }
        }
    }
}

impl TryFrom<&types::PaymentsAuthorizeRouterData> for PaymePaymentRequest {
    type Error = error_stack::Report<errors::ConnectorError>;
    fn try_from(value: &types::PaymentsAuthorizeRouterData) -> Result<Self, Self::Error> {
        let payme_request = if value.request.mandate_id.is_some() {
            Self::MandateRequest(MandateRequest::try_from(value)?)
        } else {
            Self::PayRequest(PayRequest::try_from(value)?)
        };
        Ok(payme_request)
    }
}

impl TryFrom<&types::PaymentsSyncRouterData> for PaymeQuerySaleRequest {
    type Error = error_stack::Report<errors::ConnectorError>;
    fn try_from(value: &types::PaymentsSyncRouterData) -> Result<Self, Self::Error> {
        let seller_payme_id = PaymeAuthType::try_from(&value.connector_auth_type)?.seller_payme_id;
        Ok(Self {
            sale_payme_id: value.request.get_connector_transaction_id()?,
            seller_payme_id,
        })
    }
}

impl TryFrom<&types::RefundSyncRouterData> for PaymeQueryTransactionRequest {
    type Error = error_stack::Report<errors::ConnectorError>;
    fn try_from(value: &types::RefundSyncRouterData) -> Result<Self, Self::Error> {
        let seller_payme_id = PaymeAuthType::try_from(&value.connector_auth_type)?.seller_payme_id;
        Ok(Self {
            payme_transaction_id: value
                .request
                .connector_refund_id
                .clone()
                .ok_or(errors::ConnectorError::MissingConnectorRefundID)?,
            seller_payme_id,
        })
    }
}

impl TryFrom<&types::PaymentsAuthorizeRouterData> for MandateRequest {
    type Error = error_stack::Report<errors::ConnectorError>;
    fn try_from(item: &types::PaymentsAuthorizeRouterData) -> Result<Self, Self::Error> {
        let seller_payme_id = PaymeAuthType::try_from(&item.connector_auth_type)?.seller_payme_id;
        let order_details = item.request.get_order_details()?;
        let product_name = order_details
            .first()
            .ok_or_else(missing_field_err("order_details"))?
            .product_name
            .clone();
        Ok(Self {
            currency: item.request.currency,
            sale_price: item.request.amount,
            transaction_id: item.payment_id.clone(),
            product_name,
            sale_return_url: item.request.get_return_url()?,
            seller_payme_id,
            sale_callback_url: item.request.get_webhook_url()?,
            buyer_key: Secret::new(item.request.get_connector_mandate_id()?),
        })
    }
}

impl TryFrom<&types::PaymentsAuthorizeRouterData> for PayRequest {
    type Error = error_stack::Report<errors::ConnectorError>;
    fn try_from(item: &types::PaymentsAuthorizeRouterData) -> Result<Self, Self::Error> {
        match item.request.payment_method_data.clone() {
            api::PaymentMethodData::Card(req_card) => {
                let card = PaymeCard {
                    credit_card_cvv: req_card.card_cvc.clone(),
                    credit_card_exp: req_card
                        .get_card_expiry_month_year_2_digit_with_delimiter("".to_string()),
                    credit_card_number: req_card.card_number,
                };
                let buyer_email = item.request.get_email()?;
                let buyer_name = item.get_billing_address()?.get_full_name()?;
                let payme_sale_id = item.request.related_transaction_id.clone().ok_or(
                    errors::ConnectorError::MissingConnectorRelatedTransactionID {
                        id: "payme_sale_id".to_string(),
                    },
                )?;
                Ok(Self {
                    card,
                    buyer_email,
                    buyer_name,
                    payme_sale_id,
                })
            }
            _ => Err(errors::ConnectorError::NotImplemented("Payment methods".to_string()).into()),
        }
    }
}

// Auth Struct
pub struct PaymeAuthType {
    #[allow(dead_code)]
    pub(super) payme_client_key: Secret<String>,
    pub(super) seller_payme_id: Secret<String>,
}

impl TryFrom<&types::ConnectorAuthType> for PaymeAuthType {
    type Error = error_stack::Report<errors::ConnectorError>;
    fn try_from(auth_type: &types::ConnectorAuthType) -> Result<Self, Self::Error> {
        match auth_type {
            types::ConnectorAuthType::BodyKey { api_key, key1 } => Ok(Self {
                seller_payme_id: api_key.to_owned(),
                payme_client_key: key1.to_owned(),
            }),
            _ => Err(errors::ConnectorError::FailedToObtainAuthType.into()),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SaleStatus {
    Initial,
    Completed,
    Refunded,
    PartialRefund,
    Authorized,
    Voided,
    PartialVoid,
    Failed,
    Chargeback,
}

impl From<SaleStatus> for enums::AttemptStatus {
    fn from(item: SaleStatus) -> Self {
        match item {
            SaleStatus::Initial => Self::Authorizing,
            SaleStatus::Completed => Self::Charged,
            SaleStatus::Refunded | SaleStatus::PartialRefund => Self::AutoRefunded,
            SaleStatus::Authorized => Self::Authorized,
            SaleStatus::Voided | SaleStatus::PartialVoid => Self::Voided,
            SaleStatus::Failed => Self::Failure,
            SaleStatus::Chargeback => Self::AutoRefunded,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum PaymePaymentsResponse {
    PaymePaySaleResponse(PaymePaySaleResponse),
    SaleQueryResponse(SaleQueryResponse),
}

#[derive(Clone, Debug, Deserialize)]
pub struct SaleQueryResponse {
    items: Vec<SaleQuery>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SaleQuery {
    sale_status: SaleStatus,
    sale_payme_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PaymePaySaleResponse {
    sale_status: SaleStatus,
    payme_sale_id: String,
    payme_transaction_id: String,
    buyer_key: Option<Secret<String>>,
}

#[derive(Serialize, Deserialize)]
pub struct PaymeMetadata {
    payme_transaction_id: String,
}

impl<F>
    TryFrom<
        types::ResponseRouterData<
            F,
            GenerateSaleResponse,
            types::PaymentsAuthorizeData,
            types::PaymentsResponseData,
        >,
    > for types::RouterData<F, types::PaymentsAuthorizeData, types::PaymentsResponseData>
{
    type Error = error_stack::Report<errors::ConnectorError>;
    fn try_from(
        item: types::ResponseRouterData<
            F,
            GenerateSaleResponse,
            types::PaymentsAuthorizeData,
            types::PaymentsResponseData,
        >,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            status: enums::AttemptStatus::Authorizing,
            request: types::PaymentsAuthorizeData {
                related_transaction_id: Some(item.response.payme_sale_id.clone()),
                ..item.data.request
            },
            response: Ok(types::PaymentsResponseData::TransactionResponse {
                resource_id: types::ResponseId::ConnectorTransactionId(item.response.payme_sale_id),
                redirection_data: None,
                mandate_reference: None,
                connector_metadata: None,
                network_txn_id: None,
                connector_response_reference_id: None,
            }),
            ..item.data
        })
    }
}

#[derive(Debug, Serialize)]
pub struct PaymentCaptureRequest {
    payme_sale_id: String,
    sale_price: i64,
}

impl TryFrom<&types::PaymentsCaptureRouterData> for PaymentCaptureRequest {
    type Error = error_stack::Report<errors::ConnectorError>;
    fn try_from(item: &types::PaymentsCaptureRouterData) -> Result<Self, Self::Error> {
        Ok(Self {
            payme_sale_id: item.request.connector_transaction_id.clone(),
            sale_price: item.request.amount_to_capture,
        })
    }
}

// REFUND :
// Type definition for RefundRequest
#[derive(Debug, Serialize)]
pub struct PaymeRefundRequest {
    sale_refund_amount: i64,
    payme_sale_id: String,
    seller_payme_id: Secret<String>,
}

impl<F> TryFrom<&types::RefundsRouterData<F>> for PaymeRefundRequest {
    type Error = error_stack::Report<errors::ConnectorError>;
    fn try_from(item: &types::RefundsRouterData<F>) -> Result<Self, Self::Error> {
        let auth_type = PaymeAuthType::try_from(&item.connector_auth_type)?;
        Ok(Self {
            payme_sale_id: item.request.connector_transaction_id.clone(),
            seller_payme_id: auth_type.seller_payme_id,
            sale_refund_amount: item.request.refund_amount,
        })
    }
}

impl TryFrom<SaleStatus> for enums::RefundStatus {
    type Error = error_stack::Report<errors::ConnectorError>;
    fn try_from(sale_status: SaleStatus) -> Result<Self, Self::Error> {
        match sale_status {
            SaleStatus::Refunded | SaleStatus::PartialRefund => Ok(Self::Success),
            SaleStatus::Failed => Ok(Self::Failure),
            SaleStatus::Initial
            | SaleStatus::Completed
            | SaleStatus::Authorized
            | SaleStatus::Voided
            | SaleStatus::PartialVoid
            | SaleStatus::Chargeback => Err(errors::ConnectorError::ResponseHandlingFailed)?,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct PaymeRefundResponse {
    sale_status: SaleStatus,
    payme_transaction_id: String,
}

impl TryFrom<types::RefundsResponseRouterData<api::Execute, PaymeRefundResponse>>
    for types::RefundsRouterData<api::Execute>
{
    type Error = error_stack::Report<errors::ConnectorError>;
    fn try_from(
        item: types::RefundsResponseRouterData<api::Execute, PaymeRefundResponse>,
    ) -> Result<Self, Self::Error> {
        Ok(Self {
            response: Ok(types::RefundsResponseData {
                connector_refund_id: item.response.payme_transaction_id,
                refund_status: enums::RefundStatus::try_from(item.response.sale_status)?,
            }),
            ..item.data
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PaymeQueryTransactionResponse {
    items: Vec<TransactionQuery>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TransactionQuery {
    sale_status: SaleStatus,
    payme_transaction_id: String,
}

impl<F, T>
    TryFrom<
        types::ResponseRouterData<F, PaymeQueryTransactionResponse, T, types::RefundsResponseData>,
    > for types::RouterData<F, T, types::RefundsResponseData>
{
    type Error = error_stack::Report<errors::ConnectorError>;
    fn try_from(
        item: types::ResponseRouterData<
            F,
            PaymeQueryTransactionResponse,
            T,
            types::RefundsResponseData,
        >,
    ) -> Result<Self, Self::Error> {
        let pay_sale_response = item
            .response
            .items
            .first()
            .ok_or(errors::ConnectorError::ResponseHandlingFailed)?;
        Ok(Self {
            response: Ok(types::RefundsResponseData {
                refund_status: enums::RefundStatus::try_from(
                    pay_sale_response.sale_status.clone(),
                )?,
                connector_refund_id: pay_sale_response.payme_transaction_id.clone(),
            }),
            ..item.data
        })
    }
}

#[derive(Default, Debug, Serialize, Deserialize, PartialEq)]
pub struct PaymeErrorResponse {
    pub status_code: u16,
    pub code: String,
    pub message: String,
    pub reason: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NotifyType {
    SaleComplete,
    SaleAuthorized,
    Refund,
    SaleFailure,
    SaleChargeback,
    SaleChargebackRefund,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct WebhookEventDataResource {
    pub sale_status: SaleStatus,
    pub payme_signature: Secret<String>,
    pub buyer_key: Option<Secret<String>>,
    pub notify_type: NotifyType,
    pub payme_sale_id: String,
    pub payme_transaction_id: String,
}

#[derive(Debug, Deserialize)]
pub struct WebhookEventDataResourceEvent {
    pub notify_type: NotifyType,
}

#[derive(Debug, Deserialize)]
pub struct WebhookEventDataResourceSignature {
    pub payme_signature: Secret<String>,
}

/// This try_from will ensure that webhook body would be properly parsed into PSync response
impl From<WebhookEventDataResource> for PaymePaySaleResponse {
    fn from(value: WebhookEventDataResource) -> Self {
        Self {
            sale_status: value.sale_status,
            payme_sale_id: value.payme_sale_id,
            payme_transaction_id: value.payme_transaction_id,
            buyer_key: value.buyer_key,
        }
    }
}

/// This try_from will ensure that webhook body would be properly parsed into RSync response
impl From<WebhookEventDataResource> for PaymeQueryTransactionResponse {
    fn from(value: WebhookEventDataResource) -> Self {
        let item = TransactionQuery {
            sale_status: value.sale_status,
            payme_transaction_id: value.payme_transaction_id,
        };
        Self { items: vec![item] }
    }
}

impl From<NotifyType> for api::IncomingWebhookEvent {
    fn from(value: NotifyType) -> Self {
        match value {
            NotifyType::SaleComplete => Self::PaymentIntentSuccess,
            NotifyType::Refund => Self::RefundSuccess,
            NotifyType::SaleFailure => Self::PaymentIntentFailure,
            NotifyType::SaleAuthorized
            | NotifyType::SaleChargeback
            | NotifyType::SaleChargebackRefund => Self::EventNotSupported,
        }
    }
}
