use super::parse::parse_log_str;
use crate::logs::find_json_ranges;
use bytes::{Bytes, BytesMut};
use futures::{Stream, TryStreamExt};
use anyhow::Result;

pub async fn parse_kube_stream(
    mut stream: impl Stream<Item = kube::Result<Bytes>> + std::marker::Unpin,
) -> Result<()> {
    let mut buffer = BytesMut::new();

    while let Some(line) = stream.try_next().await? {
        let mut it = line.split(|n| *n == b'\n').peekable();
        let is_last = line.ends_with(&[b'\n']);

        while let Some(chunk) = it.next() {
            buffer.extend_from_slice(chunk);

            let str = String::from_utf8_lossy(&buffer);
            let r = find_json_ranges(&str);

            if (it.peek().is_some() || is_last) && r.is_ok() {
                parse_log_str(&str)?;
                buffer.clear();
            }
        }
    }

    Ok(())
}
