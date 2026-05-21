use std::io::Read;

use crate::error::{Error, Result};
use crate::transport::blocking_transport::{BlockingResponse, BlockingResponseBody};

pub(crate) fn read_body_bytes(body: BlockingResponseBody) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    body.into_reader()
        .read_to_end(&mut out)
        .map_err(|e| Error::transport("failed to read response body", Some(Box::new(e))))?;
    Ok(out)
}

pub(crate) fn read_body_string(body: BlockingResponseBody) -> Result<String> {
    let bytes = read_body_bytes(body)?;
    crate::util::text::decode_utf8_response_body(&bytes)
}

pub(crate) fn read_response_error(resp: BlockingResponse) -> Result<Error> {
    let (parts, body) = resp.into_parts();
    let body = read_body_string(body)?;
    Ok(crate::transport::blocking_transport::response_error(
        parts.status,
        &parts.headers,
        &body,
    ))
}

pub(crate) fn parse_blocking_xml_response<T>(
    resp: BlockingResponse,
    parse: impl FnOnce(&str) -> Result<T>,
) -> Result<T> {
    let (parts, body) = resp.into_parts();
    let body = read_body_string(body)?;
    super::common::parse_xml_or_service_error(parts.status, &parts.headers, &body, parse)
}
