// @generated-stub
use stem_cell::system_api::*;

#[test]
fn payment_provider_create_charge_io_roundtrips() {
    let input = PaymentProviderCreateChargeInput {
        amount_cents: 100,
        currency: "test".to_string(),
        reference: "test".to_string(),
    };
    let _ = format!("{input:?}");

    let output = PaymentProviderCreateChargeOutput {
        charge_id: "test".to_string(),
        status: "test".to_string(),
    };
    let _ = format!("{output:?}");
}
