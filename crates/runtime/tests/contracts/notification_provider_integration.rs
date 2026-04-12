// @generated-stub
use stem_cell::system_api::*;

#[test]
fn notification_provider_send_email_io_roundtrips() {
    let input = NotificationProviderSendEmailInput {
        to: "test".to_string(),
        subject: "test".to_string(),
        body: "test".to_string(),
    };
    let _ = format!("{input:?}");

    let output = NotificationProviderSendEmailOutput {
        message_id: "test".to_string(),
    };
    let _ = format!("{output:?}");
}
