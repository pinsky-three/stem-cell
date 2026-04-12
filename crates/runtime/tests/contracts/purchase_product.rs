// @generated-stub — safe to edit; will not be overwritten if this marker is removed
use stem_cell::system_api::*;

#[test]
fn purchase_product_input_roundtrips_json() {
    let input = PurchaseProductInput {
        buyer_id: uuid::Uuid::new_v4(),
        product_id: uuid::Uuid::new_v4(),
        quantity: 1,
        payment_method_token: "test".to_string(),
        coupon_code: Some("test".to_string()),
    };
    let json = serde_json::to_string(&input).unwrap();
    let decoded: PurchaseProductInput = serde_json::from_str(&json).unwrap();
    let _ = decoded;
}

#[test]
fn purchase_product_output_roundtrips_json() {
    let output = PurchaseProductOutput {
        order_id: uuid::Uuid::new_v4(),
        payment_status: "test".to_string(),
        total_cents: 100,
    };
    let json = serde_json::to_string(&output).unwrap();
    let decoded: PurchaseProductOutput = serde_json::from_str(&json).unwrap();
    let _ = decoded;
}

#[test]
fn purchase_product_internal_error_converts() {
    let e = PurchaseProductError::Internal("oops".into());
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("internal"), "expected 'internal' in '{msg}'");
}

#[test]
fn error_buyer_not_found_converts_to_system_error() {
    let e = PurchaseProductError::BuyerNotFound;
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("BuyerNotFound"), "expected 'BuyerNotFound' in '{msg}'");
}

#[test]
fn error_product_not_found_converts_to_system_error() {
    let e = PurchaseProductError::ProductNotFound;
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("ProductNotFound"), "expected 'ProductNotFound' in '{msg}'");
}

#[test]
fn error_insufficient_stock_converts_to_system_error() {
    let e = PurchaseProductError::InsufficientStock;
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("InsufficientStock"), "expected 'InsufficientStock' in '{msg}'");
}

#[test]
fn error_payment_failed_converts_to_system_error() {
    let e = PurchaseProductError::PaymentFailed("test".into());
    let se: SystemError = e.into();
    let msg = format!("{se}");
    assert!(msg.contains("PaymentFailed"), "expected 'PaymentFailed' in '{msg}'");
}
