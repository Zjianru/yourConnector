//! 会话传输层：统一 envelope 下发。

use anyhow::Result;
use futures_util::Sink;
use serde_json::Value;
use tokio_tungstenite::tungstenite::Message;
use yc_shared_protocol::{EventEnvelope, now_rfc3339_nanos};

/// 发送标准 envelope 事件，并维护单连接内递增 seq。
pub(crate) async fn send_event<W>(
    ws_writer: &mut W,
    system_id: &str,
    seq: &mut u64,
    event_type: &str,
    payload: Value,
) -> Result<()>
where
    W: Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    *seq += 1;
    let mut env = EventEnvelope::new(event_type, system_id, payload);
    env.seq = Some(*seq);
    env.ts = now_rfc3339_nanos();

    let raw = serde_json::to_string(&env)?;
    futures_util::SinkExt::send(ws_writer, Message::Text(raw.into())).await?;
    Ok(())
}
