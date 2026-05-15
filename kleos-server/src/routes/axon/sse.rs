//! SSE streaming endpoint for real-time Axon event delivery.
//!
//! Uses a polling approach: queries for new events every 2 seconds.
//! Clients connect with channel list and optional type filter, and
//! receive events as they are published.

use axum::extract::Query;
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use futures::stream::Stream;
use std::convert::Infallible;
use std::time::Duration;

use crate::error::AppError;
use crate::extractors::{Auth, ResolvedDb};
use kleos_lib::services::axon::query_events;

use super::types::SseStreamParams;

/// SSE stream handler. Polls for new Axon events every 2 seconds
/// and delivers them to the connected client.
pub async fn stream_handler(
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Query(params): Query<SseStreamParams>,
) -> Result<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>, AppError> {
    let channels: Vec<String> = params
        .channels
        .map(|c| c.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_default();
    let filter_type = params.filter_type.clone();
    let mut last_id = params.last_event_id.unwrap_or(0);

    let stream = async_stream::stream! {
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        loop {
            interval.tick().await;

            for channel in &channels {
                let events = match query_events(
                    &db,
                    Some(channel),
                    filter_type.as_deref(),
                    None,
                    100,
                    0,
                    auth.user_id,
                )
                .await
                {
                    Ok(evts) => evts,
                    Err(_) => continue,
                };

                for event in events {
                    if event.id <= last_id {
                        continue;
                    }
                    last_id = event.id;
                    let data = serde_json::json!({
                        "id": event.id,
                        "channel": event.channel,
                        "action": event.action,
                        "payload": event.payload,
                        "source": event.source,
                        "created_at": event.created_at,
                    });
                    yield Ok(SseEvent::default()
                        .id(event.id.to_string())
                        .event(event.action.clone())
                        .data(data.to_string()));
                }
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(30))
            .text("keep-alive"),
    ))
}
