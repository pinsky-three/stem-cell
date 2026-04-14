use crate::error::{Error, Result};
use crate::types::OpenCodeEvent;
use futures::stream::Stream;
use reqwest_eventsource::{Event, EventSource};
use std::time::Duration;

/// Opens an SSE connection to the OpenCode `/event` endpoint and yields
/// typed `OpenCodeEvent` values. The stream ends when the server closes
/// the connection or an unrecoverable error occurs.
///
/// Takes owned strings so the returned stream is `'static` and can be
/// moved into `tokio::spawn`.
pub fn subscribe(
    base_url: String,
    auth_header: Option<String>,
) -> Result<impl Stream<Item = Result<OpenCodeEvent>> + Send + Unpin + 'static> {
    let url = format!("{base_url}/event");

    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .no_proxy()
        .build()
        .map_err(|e| Error::SseError(e.to_string()))?;

    let mut builder = http.get(&url);
    if let Some(ref auth) = auth_header {
        builder = builder.header(reqwest::header::AUTHORIZATION, auth.as_str());
    }

    let mut es = EventSource::new(builder).map_err(|e| Error::SseError(e.to_string()))?;
    es.set_retry_policy(Box::new(reqwest_eventsource::retry::Never));

    Ok(SseStream { es })
}

/// Wrapper that implements `Stream<Item = Result<OpenCodeEvent>>`.
struct SseStream {
    es: EventSource,
}

impl Stream for SseStream {
    type Item = Result<OpenCodeEvent>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        use futures::stream::StreamExt;
        loop {
            match self.as_mut().es.poll_next_unpin(cx) {
                // Skip synthetic "open": we already get `server.connected` (or equivalent) on the wire;
                // emitting both duplicated "SSE connected" logs in the host.
                std::task::Poll::Ready(Some(Ok(Event::Open))) => continue,
                std::task::Poll::Ready(Some(Ok(Event::Message(msg)))) => {
                    let event = OpenCodeEvent::parse(&msg.event, &msg.data);
                    return std::task::Poll::Ready(Some(Ok(event)));
                }
                std::task::Poll::Ready(Some(Err(e))) => {
                    if matches!(e, reqwest_eventsource::Error::StreamEnded) {
                        return std::task::Poll::Ready(None);
                    }
                    return std::task::Poll::Ready(Some(Err(Error::SseError(e.to_string()))));
                }
                std::task::Poll::Ready(None) => return std::task::Poll::Ready(None),
                std::task::Poll::Pending => return std::task::Poll::Pending,
            }
        }
    }
}

// Safety: EventSource is Send and so is our wrapper
unsafe impl Send for SseStream {}
