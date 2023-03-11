use super::parse::parse_log_str;
use bytes::BytesMut;
use futures::{Stream, TryStreamExt};
use bollard::container::LogOutput;
use bollard::errors::Error;
use anyhow::Result;

const DOCKER_BUFFER_SIZE: usize = 8192;

pub async fn parse_docker_stream(
    mut stream: impl Stream<Item = core::result::Result<LogOutput, Error>>
        + std::marker::Unpin,
) -> Result<()> {
    let mut buffer = BytesMut::new();

    while let Some(line) = stream.try_next().await?.map(|logs| logs.into_bytes()) {
        let is_last = line.len() < DOCKER_BUFFER_SIZE;

        if let Some((_, chunk)) = line.split_last() {
            buffer.extend_from_slice(&chunk);

            if is_last && !buffer.is_empty() {
                for str in String::from_utf8_lossy(&buffer).split(|c| c == '\n') {
                    parse_log_str(&str)?;
                }
                buffer.clear();
            }
        }
    }

    Ok(())
}
