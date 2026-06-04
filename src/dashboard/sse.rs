#[cfg(feature = "dashboard")]
pub mod imp {
    use axum::response::sse::Event;
    use tokio::sync::broadcast;
    use tokio_stream::{wrappers::BroadcastStream, StreamExt};
    use crate::state::watcher::StateEvent;

    pub type SseSender = broadcast::Sender<StateEvent>;

    pub fn to_sse_stream(
        rx: broadcast::Receiver<StateEvent>,
    ) -> impl tokio_stream::Stream<Item = Result<Event, std::convert::Infallible>> {
        BroadcastStream::new(rx).filter_map(|msg| {
            msg.ok().map(|event| {
                let data = match &event {
                    StateEvent::TaskChanged(id) => format!("{{\"type\":\"task\",\"id\":\"{id}\"}}"),
                    StateEvent::WipChanged => r#"{"type":"wip"}"#.to_string(),
                    StateEvent::AnyChange => r#"{"type":"change"}"#.to_string(),
                };
                Ok(Event::default().data(data))
            })
        })
    }
}
