//! SSE streaming endpoint for real-time Axon event delivery.
//!
//! Uses a broadcast channel for immediate push delivery. On connect,
//! replays any missed events since last_event_id, then switches to
//! real-time broadcast reception.
//!
//! SECURITY: The broadcast channel is process-wide. Tenant isolation is
//! enforced by filtering on user_id -- each SSE subscriber only receives
//! events published by the same user. The catch-up query is already
//! tenant-scoped via ResolvedDb shard isolation.

use axum::extract::{Query, State};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use futures::stream::Stream;
use std::convert::Infallible;
use std::time::Duration;
use tokio::sync::broadcast;

use crate::error::AppError;
use crate::extractors::{Auth, ResolvedDb};
use crate::state::AppState;
use kleos_lib::services::axon::query_events;

use super::types::SseStreamParams;

/// SSE stream handler. Delivers Axon events in real-time via broadcast channel.
/// On connect, replays missed events since last_event_id, then streams live.
pub async fn stream_handler(
    State(state): State<AppState>,
    ResolvedDb(db): ResolvedDb,
    Auth(auth): Auth,
    Query(params): Query<SseStreamParams>,
) -> Result<Sse<impl Stream<Item = Result<SseEvent, Infallible>>>, AppError> {
    // Parse the channels CSV. A literal "*" entry (or an entirely empty channel
    // list) means "subscribe to every channel"; in that mode the inner loop
    // queries with channel=None so SQL returns events across all channels.
    let raw_channels: Vec<String> = params
        .channels
        .map(|c| {
            c.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let wildcard = raw_channels.is_empty() || raw_channels.iter().any(|s| s == "*");
    let channels = raw_channels;
    let filter_type = params.filter_type.clone();
    let mut last_id = params.last_event_id.unwrap_or(0);
    let subscriber_user_id = auth.user_id;

    // Subscribe BEFORE catch-up query so the broadcast buffers any events
    // published during the DB round-trip. The ev_id <= last_id guard in the
    // stream loop deduplicates overlap.
    let mut rx = state.axon_broadcast.subscribe();

    // Catch-up: replay missed events from DB
    let catchup_events = if last_id > 0 {
        let channel_filter = if wildcard {
            None
        } else {
            channels.first().map(|s| s.as_str())
        };
        query_events(
            &db,
            channel_filter,
            filter_type.as_deref(),
            None,
            1000,
            0,
            auth.user_id,
        )
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|e| e.id > last_id)
        .collect::<Vec<_>>()
    } else {
        Vec::new()
    };

    let stream = async_stream::stream! {
        // Phase 1: Deliver catch-up events
        for event in catchup_events {
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

        // Phase 2: Real-time broadcast delivery
        loop {
            match rx.recv().await {
                Ok(event_json) => {
                    // Extract fields for filtering
                    let ev_channel = event_json.get("channel")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let ev_action = event_json.get("action")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let ev_id = event_json.get("id")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);

                    // Tenant isolation: only deliver events from same user
                    let ev_user_id = event_json.get("user_id")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    if ev_user_id != subscriber_user_id {
                        continue;
                    }

                    // Skip if already delivered in catch-up
                    if ev_id <= last_id {
                        continue;
                    }

                    // Channel filter
                    if !wildcard && !channels.iter().any(|c| c == ev_channel) {
                        continue;
                    }

                    // Type/action filter
                    if let Some(ref ft) = filter_type {
                        if ft != ev_action {
                            continue;
                        }
                    }

                    last_id = ev_id;
                    yield Ok(SseEvent::default()
                        .id(ev_id.to_string())
                        .event(ev_action)
                        .data(event_json.to_string()));
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(lagged = n, "SSE client lagged behind broadcast");
                    // Continue receiving -- client missed some events but can use
                    // last_event_id on reconnect to catch up
                    continue;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    break;
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
