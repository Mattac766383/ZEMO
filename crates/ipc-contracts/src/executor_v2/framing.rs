use super::{
    AuthenticatedOperationRequest, AuthenticatedOperationResponse, HandshakeRefusal, Hello,
    MAX_FRAME_BYTES, OpenSession, SessionOpened,
};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::io::{self, Read, Write};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "payload",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum CoordinatorHandshakeFrame {
    OpenSession(OpenSession),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "payload",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum CoordinatorSessionFrame {
    ExecuteOperation(AuthenticatedOperationRequest),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "payload",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum ExecutorFrame {
    Hello(Hello),
    SessionOpened(SessionOpened),
    HandshakeRefusal(HandshakeRefusal),
    OperationResponse(AuthenticatedOperationResponse),
}

pub fn write_frame<W, T>(writer: &mut W, value: &T) -> Result<(), FrameError>
where
    W: Write,
    T: Serialize,
{
    let payload = serde_json::to_vec(value).map_err(|_| FrameError::Encode)?;
    if payload.is_empty() || payload.len() > MAX_FRAME_BYTES {
        return Err(FrameError::Oversized);
    }
    let length = u32::try_from(payload.len()).map_err(|_| FrameError::Oversized)?;
    writer.write_all(&length.to_be_bytes())?;
    writer.write_all(&payload)?;
    writer.flush()?;
    Ok(())
}

pub fn read_frame<R, T>(reader: &mut R) -> Result<Option<T>, FrameError>
where
    R: Read,
    T: DeserializeOwned,
{
    let mut header = [0_u8; 4];
    loop {
        match reader.read(&mut header[..1]) {
            Ok(0) => return Ok(None),
            Ok(1) => break,
            Ok(_) => unreachable!(),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(FrameError::Io(error)),
        }
    }
    reader
        .read_exact(&mut header[1..])
        .map_err(|error| match error.kind() {
            io::ErrorKind::UnexpectedEof => FrameError::Truncated,
            _ => FrameError::Io(error),
        })?;
    let length = usize::try_from(u32::from_be_bytes(header)).map_err(|_| FrameError::Oversized)?;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(FrameError::Oversized);
    }
    let mut payload = vec![0_u8; length];
    reader
        .read_exact(&mut payload)
        .map_err(|error| match error.kind() {
            io::ErrorKind::UnexpectedEof => FrameError::Truncated,
            _ => FrameError::Io(error),
        })?;
    serde_json::from_slice(&payload)
        .map(Some)
        .map_err(|_| FrameError::Decode)
}

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("executor frame exceeds the hard size limit")]
    Oversized,
    #[error("executor frame ended before its declared length")]
    Truncated,
    #[error("executor frame contains invalid or unknown fields")]
    Decode,
    #[error("executor frame could not be encoded")]
    Encode,
    #[error("executor pipe I/O failed: {0}")]
    Io(#[from] io::Error),
}
