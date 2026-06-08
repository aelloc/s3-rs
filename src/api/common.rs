use http::{
    HeaderMap, HeaderValue, StatusCode,
    header::{HeaderName, IntoHeaderName},
};

#[cfg(feature = "multipart")]
use crate::types::CompletedPart;
use crate::{
    error::{Error, Result},
    types::{DeleteObjectIdentifier, MAX_DELETE_OBJECTS_PER_REQUEST},
};

pub(crate) const MAX_LIST_OBJECTS_KEYS: u32 = 1_000;
#[cfg(feature = "multipart")]
pub(crate) const MAX_LIST_PARTS: u32 = 1_000;
#[cfg(feature = "multipart")]
pub(crate) const MAX_UPLOAD_PART_NUMBER: u32 = 10_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ByteRange {
    start: u64,
    end_inclusive: u64,
}

impl ByteRange {
    pub(crate) fn new(start: u64, end_inclusive: u64) -> Result<Self> {
        if start > end_inclusive {
            return Err(Error::invalid_config(
                "byte range start must be <= end_inclusive",
            ));
        }

        Ok(Self {
            start,
            end_inclusive,
        })
    }

    pub(crate) fn header_value(self, invalid_message: &'static str) -> Result<HeaderValue> {
        header_value(
            format!("bytes={}-{}", self.start, self.end_inclusive),
            invalid_message,
        )
    }
}

pub(crate) fn header_value(
    value: impl AsRef<str>,
    invalid_message: &'static str,
) -> Result<HeaderValue> {
    HeaderValue::from_str(value.as_ref()).map_err(|_| Error::invalid_config(invalid_message))
}

pub(crate) fn validate_header_value(value: &str, invalid_message: &'static str) -> Result<()> {
    if value.is_empty() || value.trim() != value {
        return Err(Error::invalid_config(invalid_message));
    }
    header_value(value, invalid_message)?;
    Ok(())
}

fn validate_metadata_value(value: &str) -> Result<()> {
    if value.trim() != value {
        return Err(Error::invalid_config(
            "metadata values must not include leading or trailing whitespace",
        ));
    }
    header_value(value, "invalid metadata header value")?;
    Ok(())
}

pub(crate) fn insert_header<K>(
    headers: &mut HeaderMap,
    name: K,
    value: impl AsRef<str>,
    invalid_message: &'static str,
) -> Result<()>
where
    K: IntoHeaderName,
{
    headers.insert(name, header_value(value, invalid_message)?);
    Ok(())
}

pub(crate) fn insert_optional_header(
    headers: &mut HeaderMap,
    name: HeaderName,
    value: Option<String>,
    invalid_message: &'static str,
) -> Result<()> {
    if let Some(value) = value {
        validate_header_value(&value, invalid_message)?;
        insert_header(headers, name, value, invalid_message)?;
    }
    Ok(())
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ObjectConditions {
    if_match: Option<String>,
    if_none_match: Option<String>,
}

impl ObjectConditions {
    pub(crate) fn set_if_match(&mut self, value: impl Into<String>) -> Result<()> {
        let value = value.into();
        validate_header_value(&value, "invalid If-Match header")?;
        self.if_match = Some(value);
        Ok(())
    }

    pub(crate) fn set_if_none_match(&mut self, value: impl Into<String>) -> Result<()> {
        let value = value.into();
        validate_header_value(&value, "invalid If-None-Match header")?;
        self.if_none_match = Some(value);
        Ok(())
    }

    pub(crate) fn apply(self, headers: &mut HeaderMap) -> Result<()> {
        insert_optional_header(
            headers,
            http::header::IF_MATCH,
            self.if_match,
            "invalid If-Match header",
        )?;
        insert_optional_header(
            headers,
            http::header::IF_NONE_MATCH,
            self.if_none_match,
            "invalid If-None-Match header",
        )
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct PutObjectHeaders {
    content_type: Option<String>,
    cache_control: Option<String>,
    content_disposition: Option<String>,
    content_encoding: Option<String>,
    content_language: Option<String>,
    expires: Option<String>,
    conditions: ObjectConditions,
    metadata: Vec<(String, String)>,
}

impl PutObjectHeaders {
    pub(crate) fn content_type(&mut self, value: impl Into<String>) -> Result<()> {
        self.set_optional_header(value, "invalid Content-Type header", |headers, value| {
            headers.content_type = Some(value);
        })
    }

    pub(crate) fn cache_control(&mut self, value: impl Into<String>) -> Result<()> {
        self.set_optional_header(value, "invalid Cache-Control header", |headers, value| {
            headers.cache_control = Some(value);
        })
    }

    pub(crate) fn content_disposition(&mut self, value: impl Into<String>) -> Result<()> {
        self.set_optional_header(
            value,
            "invalid Content-Disposition header",
            |headers, value| {
                headers.content_disposition = Some(value);
            },
        )
    }

    pub(crate) fn content_encoding(&mut self, value: impl Into<String>) -> Result<()> {
        self.set_optional_header(
            value,
            "invalid Content-Encoding header",
            |headers, value| {
                headers.content_encoding = Some(value);
            },
        )
    }

    pub(crate) fn content_language(&mut self, value: impl Into<String>) -> Result<()> {
        self.set_optional_header(
            value,
            "invalid Content-Language header",
            |headers, value| {
                headers.content_language = Some(value);
            },
        )
    }

    pub(crate) fn expires(&mut self, value: impl Into<String>) -> Result<()> {
        self.set_optional_header(value, "invalid Expires header", |headers, value| {
            headers.expires = Some(value);
        })
    }

    pub(crate) fn if_match(&mut self, value: impl Into<String>) -> Result<()> {
        self.conditions.set_if_match(value)
    }

    pub(crate) fn if_none_match(&mut self, value: impl Into<String>) -> Result<()> {
        self.conditions.set_if_none_match(value)
    }

    pub(crate) fn metadata(
        &mut self,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<()> {
        push_metadata(&mut self.metadata, key, value)
    }

    pub(crate) fn into_header_map(self) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();
        insert_optional_header(
            &mut headers,
            http::header::CONTENT_TYPE,
            self.content_type,
            "invalid Content-Type header",
        )?;
        insert_optional_header(
            &mut headers,
            http::header::CACHE_CONTROL,
            self.cache_control,
            "invalid Cache-Control header",
        )?;
        insert_optional_header(
            &mut headers,
            http::header::CONTENT_DISPOSITION,
            self.content_disposition,
            "invalid Content-Disposition header",
        )?;
        insert_optional_header(
            &mut headers,
            http::header::CONTENT_ENCODING,
            self.content_encoding,
            "invalid Content-Encoding header",
        )?;
        insert_optional_header(
            &mut headers,
            http::header::CONTENT_LANGUAGE,
            self.content_language,
            "invalid Content-Language header",
        )?;
        insert_optional_header(
            &mut headers,
            http::header::EXPIRES,
            self.expires,
            "invalid Expires header",
        )?;
        self.conditions.apply(&mut headers)?;
        apply_metadata_headers(&mut headers, self.metadata)?;
        Ok(headers)
    }

    fn set_optional_header(
        &mut self,
        value: impl Into<String>,
        invalid_message: &'static str,
        set: impl FnOnce(&mut Self, String),
    ) -> Result<()> {
        let value = value.into();
        validate_header_value(&value, invalid_message)?;
        set(self, value);
        Ok(())
    }
}

pub(crate) fn xml_body_headers(body: &[u8]) -> Result<HeaderMap> {
    let mut headers = HeaderMap::new();
    headers.insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("application/xml"),
    );
    headers.insert(
        HeaderName::from_static("content-md5"),
        crate::util::md5::content_md5_header_value(body)?,
    );
    Ok(headers)
}

pub(crate) fn require_configured<T>(value: Option<T>, message: &'static str) -> Result<T> {
    value.ok_or_else(|| Error::invalid_config(message))
}

pub(crate) fn validate_content_length_matches_body(
    configured: Option<u64>,
    body_len: usize,
    context: &'static str,
) -> Result<u64> {
    let body_len = u64::try_from(body_len)
        .map_err(|_| Error::invalid_config(format!("{context} body length exceeds u64")))?;
    if let Some(configured) = configured
        && configured != body_len
    {
        return Err(Error::invalid_config(format!(
            "{context} content_length must match the byte body length"
        )));
    }
    Ok(body_len)
}

pub(crate) fn parse_xml_or_service_error<T>(
    status: StatusCode,
    headers: &HeaderMap,
    body: &str,
    parse: impl FnOnce(&str) -> Result<T>,
) -> Result<T> {
    match parse(body) {
        Ok(value) => Ok(value),
        Err(parse_error) => {
            if crate::util::xml::parse_error_xml(body).is_some() {
                return Err(crate::transport::response_error_from_status(
                    status, headers, body,
                ));
            }
            Err(parse_error)
        }
    }
}

#[cfg(feature = "async")]
pub(crate) fn parse_async_xml_response<T>(
    resp: crate::transport::async_transport::AsyncResponse,
    parse: impl FnOnce(&str) -> Result<T>,
) -> Result<T> {
    let (status, headers, body) = resp.into_parts();
    let body = crate::util::text::decode_utf8_response_body(body.as_ref())?;
    parse_xml_or_service_error(status, &headers, &body, parse)
}

pub(crate) fn create_bucket_location_constraint(
    explicit: Option<String>,
    client_region: &str,
) -> Result<Option<String>> {
    match explicit {
        Some(region) => {
            crate::auth::Region::new(region.as_str())?;
            Ok(Some(region))
        }
        None => {
            crate::auth::Region::new(client_region)?;
            if client_region == "us-east-1" {
                Ok(None)
            } else {
                Ok(Some(client_region.to_string()))
            }
        }
    }
}

pub(crate) fn validate_max_keys(max_keys: u32) -> Result<()> {
    if max_keys == 0 || max_keys > MAX_LIST_OBJECTS_KEYS {
        return Err(Error::invalid_config(
            "max_keys must be in the range 1..=1000",
        ));
    }
    Ok(())
}

pub(crate) fn next_list_v2_continuation_token(
    current: Option<&str>,
    next: Option<&str>,
    is_truncated: bool,
) -> Result<Option<String>> {
    if !is_truncated {
        return Ok(None);
    }

    let next = next.ok_or_else(|| {
        Error::decode(
            "ListObjectsV2 response is truncated but missing NextContinuationToken",
            None,
        )
    })?;
    validate_query_token("next_continuation_token", next).map_err(|_| {
        Error::decode(
            "ListObjectsV2 response contains an invalid NextContinuationToken",
            None,
        )
    })?;

    if current.is_some_and(|current| current == next) {
        return Err(Error::decode(
            "ListObjectsV2 response repeated the current continuation token",
            None,
        ));
    }

    Ok(Some(next.to_string()))
}

#[cfg(feature = "multipart")]
pub(crate) fn validate_max_parts(max_parts: u32) -> Result<()> {
    if max_parts == 0 || max_parts > MAX_LIST_PARTS {
        return Err(Error::invalid_config(
            "max_parts must be in the range 1..=1000",
        ));
    }
    Ok(())
}

#[cfg(feature = "multipart")]
pub(crate) fn validate_part_number_marker(part_number_marker: u32) -> Result<()> {
    if part_number_marker == 0 || part_number_marker > MAX_UPLOAD_PART_NUMBER {
        return Err(Error::invalid_config(
            "part_number_marker must be in the range 1..=10000",
        ));
    }
    Ok(())
}

#[cfg(feature = "multipart")]
pub(crate) fn validate_upload_part_number(part_number: u32) -> Result<()> {
    if part_number == 0 || part_number > MAX_UPLOAD_PART_NUMBER {
        return Err(Error::invalid_config(
            "part_number must be in the range 1..=10000",
        ));
    }
    Ok(())
}

#[cfg(feature = "multipart")]
pub(crate) fn validate_upload_id(upload_id: &str) -> Result<()> {
    validate_query_token("upload_id", upload_id)
}

pub(crate) fn validate_query_token(name: &'static str, value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(Error::invalid_config(format!("{name} must not be empty")));
    }
    if value.trim() != value {
        return Err(Error::invalid_config(format!(
            "{name} must not include leading or trailing whitespace"
        )));
    }
    if value
        .bytes()
        .any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
    {
        return Err(Error::invalid_config(format!(
            "{name} must not contain ASCII control or whitespace characters"
        )));
    }
    Ok(())
}

pub(crate) fn validate_query_value(name: &'static str, value: &str) -> Result<()> {
    if value.is_empty() {
        return Err(Error::invalid_config(format!("{name} must not be empty")));
    }
    if value.bytes().any(|b| b.is_ascii_control()) {
        return Err(Error::invalid_config(format!(
            "{name} must not contain ASCII control characters"
        )));
    }
    Ok(())
}

pub(crate) fn push_delete_object(
    objects: &mut Vec<DeleteObjectIdentifier>,
    object: DeleteObjectIdentifier,
) -> Result<()> {
    if objects.len() >= MAX_DELETE_OBJECTS_PER_REQUEST {
        return Err(Error::invalid_config(
            "delete_objects supports at most 1000 objects per request",
        ));
    }
    objects.push(object);
    Ok(())
}

#[cfg(feature = "multipart")]
pub(crate) fn push_completed_part(
    parts: &mut Vec<CompletedPart>,
    part: CompletedPart,
) -> Result<()> {
    if parts.len() >= MAX_UPLOAD_PART_NUMBER as usize {
        return Err(Error::invalid_config(
            "complete_multipart_upload supports at most 10000 completed parts",
        ));
    }
    if parts
        .iter()
        .any(|existing| existing.part_number() == part.part_number())
    {
        return Err(Error::invalid_config(
            "completed part numbers must be unique",
        ));
    }
    parts.push(part);
    Ok(())
}

#[cfg(feature = "multipart")]
pub(crate) fn prepare_completed_parts(mut parts: Vec<CompletedPart>) -> Result<Vec<CompletedPart>> {
    if parts.is_empty() {
        return Err(Error::invalid_config(
            "complete_multipart_upload requires at least one completed part",
        ));
    }
    if parts.len() > MAX_UPLOAD_PART_NUMBER as usize {
        return Err(Error::invalid_config(
            "complete_multipart_upload supports at most 10000 completed parts",
        ));
    }

    parts.sort_by_key(|part| part.part_number());
    if parts
        .windows(2)
        .any(|pair| pair[0].part_number() == pair[1].part_number())
    {
        return Err(Error::invalid_config(
            "completed part numbers must be unique",
        ));
    }

    Ok(parts)
}

pub(crate) fn validate_subresource(subresource: &str) -> Result<()> {
    if subresource.is_empty() {
        return Err(Error::invalid_config("subresource must not be empty"));
    }
    if subresource.trim() != subresource {
        return Err(Error::invalid_config(
            "subresource must not include leading or trailing whitespace",
        ));
    }
    if subresource
        .bytes()
        .any(|b| b.is_ascii_control() || b.is_ascii_whitespace())
    {
        return Err(Error::invalid_config(
            "subresource must not contain ASCII control or whitespace characters",
        ));
    }
    Ok(())
}

pub(crate) fn apply_metadata_headers(
    headers: &mut HeaderMap,
    metadata: Vec<(String, String)>,
) -> Result<()> {
    let mut pending = Vec::with_capacity(metadata.len());
    for (name, value) in metadata {
        let header_name = crate::util::redact::metadata_header_name(&name)?;
        if headers.contains_key(&header_name)
            || pending
                .iter()
                .any(|(existing, _): &(HeaderName, String)| existing == header_name)
        {
            return Err(Error::invalid_config("metadata keys must be unique"));
        }
        validate_metadata_value(&value)?;
        pending.push((header_name, value));
    }

    for (name, value) in pending {
        insert_header(headers, name, value, "invalid metadata header value")?;
    }
    Ok(())
}

pub(crate) fn push_metadata(
    metadata: &mut Vec<(String, String)>,
    key: impl Into<String>,
    value: impl Into<String>,
) -> Result<()> {
    let key = key.into();
    let value = value.into();
    let header_name = crate::util::redact::metadata_header_name(&key)?;
    validate_metadata_value(&value)?;

    if metadata.iter().any(|(existing, _)| {
        crate::util::redact::metadata_header_name(existing)
            .is_ok_and(|existing_header| existing_header == header_name)
    }) {
        return Err(Error::invalid_config("metadata keys must be unique"));
    }

    metadata.push((key, value));
    Ok(())
}

pub(crate) fn apply_copy_metadata_headers(
    headers: &mut HeaderMap,
    explicit_replace: bool,
    content_type: Option<String>,
    metadata: Vec<(String, String)>,
) -> Result<()> {
    let should_replace = explicit_replace || content_type.is_some() || !metadata.is_empty();
    if should_replace {
        headers.insert(
            "x-amz-metadata-directive",
            HeaderValue::from_static("REPLACE"),
        );
    }

    insert_optional_header(
        headers,
        http::header::CONTENT_TYPE,
        content_type,
        "invalid Content-Type header",
    )?;
    apply_metadata_headers(headers, metadata)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;

    #[test]
    fn create_bucket_location_constraint_defaults_to_client_region() {
        assert_eq!(
            create_bucket_location_constraint(None, "ap-southeast-1").unwrap(),
            Some("ap-southeast-1".to_string())
        );
    }

    #[test]
    fn create_bucket_location_constraint_skips_us_east_1_by_default() {
        assert_eq!(
            create_bucket_location_constraint(None, "us-east-1").unwrap(),
            None
        );
        assert!(create_bucket_location_constraint(None, "US-EAST-1").is_err());
    }

    #[test]
    fn create_bucket_location_constraint_respects_explicit_value() {
        assert_eq!(
            create_bucket_location_constraint(Some("eu-west-1".to_string()), "us-east-1").unwrap(),
            Some("eu-west-1".to_string())
        );
    }

    #[test]
    fn create_bucket_location_constraint_rejects_invalid_explicit_value() {
        let err = create_bucket_location_constraint(Some("eu west 1".to_string()), "us-east-1")
            .expect_err("invalid explicit location constraint must be rejected");
        match err {
            Error::InvalidConfig { message } => assert!(message.contains("region")),
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
    }

    #[test]
    fn validate_content_length_matches_byte_body() {
        assert_eq!(
            validate_content_length_matches_body(None, 3, "put_object").unwrap(),
            3
        );
        assert_eq!(
            validate_content_length_matches_body(Some(3), 3, "put_object").unwrap(),
            3
        );
        let err = validate_content_length_matches_body(Some(4), 3, "put_object")
            .expect_err("mismatched content length must be rejected");
        match err {
            Error::InvalidConfig { message } => assert!(message.contains("content_length")),
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
    }

    #[test]
    fn validate_max_keys_accepts_range_and_rejects_out_of_range() {
        assert!(validate_max_keys(1).is_ok());
        assert!(validate_max_keys(1_000).is_ok());
        assert!(validate_max_keys(0).is_err());
        assert!(validate_max_keys(1_001).is_err());
    }

    #[test]
    fn next_list_v2_continuation_token_rejects_invalid_truncated_pages() {
        assert_eq!(
            next_list_v2_continuation_token(None, None, false).unwrap(),
            None
        );
        assert_eq!(
            next_list_v2_continuation_token(None, Some("token-2"), true).unwrap(),
            Some("token-2".to_string())
        );
        assert!(next_list_v2_continuation_token(None, None, true).is_err());
        assert!(next_list_v2_continuation_token(Some("same"), Some("same"), true).is_err());
        assert!(next_list_v2_continuation_token(None, Some(" bad"), true).is_err());
    }

    #[cfg(feature = "multipart")]
    #[test]
    fn validate_max_parts_accepts_range_and_rejects_out_of_range() {
        assert!(validate_max_parts(1).is_ok());
        assert!(validate_max_parts(1_000).is_ok());
        assert!(validate_max_parts(0).is_err());
        assert!(validate_max_parts(1_001).is_err());
    }

    #[cfg(feature = "multipart")]
    #[test]
    fn validate_part_number_marker_accepts_range_and_rejects_out_of_range() {
        assert!(validate_part_number_marker(1).is_ok());
        assert!(validate_part_number_marker(10_000).is_ok());
        assert!(validate_part_number_marker(0).is_err());
        assert!(validate_part_number_marker(10_001).is_err());
    }

    #[test]
    fn byte_range_rejects_reversed_bounds() {
        let err = ByteRange::new(10, 9).expect_err("reversed byte range should be rejected");

        match err {
            Error::InvalidConfig { message } => assert!(message.contains("byte range")),
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
    }

    #[test]
    fn byte_range_formats_http_header_value() {
        let value = ByteRange::new(3, 9)
            .expect("range should be valid")
            .header_value("invalid Range header")
            .expect("range should be valid");
        assert_eq!(value.to_str().ok(), Some("bytes=3-9"));
    }

    #[test]
    fn insert_optional_header_rejects_empty_or_outer_whitespace() {
        for value in ["", " text/plain", "text/plain "] {
            let mut headers = HeaderMap::new();
            let err = insert_optional_header(
                &mut headers,
                http::header::CONTENT_TYPE,
                Some(value.to_string()),
                "invalid Content-Type header",
            )
            .expect_err("ambiguous header values must be rejected");

            match err {
                Error::InvalidConfig { message } => {
                    assert!(message.contains("Content-Type"));
                    assert!(headers.is_empty());
                }
                other => panic!("expected InvalidConfig, got {other:?}"),
            }
        }
    }

    #[test]
    fn object_conditions_apply_conditional_headers() {
        let mut conditions = ObjectConditions::default();
        conditions
            .set_if_match("\"etag\"")
            .expect("If-Match should be valid");
        conditions
            .set_if_none_match("*")
            .expect("If-None-Match wildcard should be valid");

        let mut headers = HeaderMap::new();
        conditions
            .apply(&mut headers)
            .expect("headers should apply");

        assert_eq!(
            headers
                .get(http::header::IF_MATCH)
                .and_then(|value| value.to_str().ok()),
            Some("\"etag\"")
        );
        assert_eq!(
            headers
                .get(http::header::IF_NONE_MATCH)
                .and_then(|value| value.to_str().ok()),
            Some("*")
        );
    }

    #[test]
    fn put_object_headers_include_conditions_and_metadata() {
        let mut headers = PutObjectHeaders::default();
        headers
            .content_type("text/plain")
            .expect("content type should be valid");
        headers
            .if_none_match("*")
            .expect("If-None-Match wildcard should be valid");
        headers
            .metadata("lease-owner", "client-1")
            .expect("metadata should be valid");

        let headers = headers.into_header_map().expect("headers should be valid");

        assert_eq!(
            headers
                .get(http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("text/plain")
        );
        assert_eq!(
            headers
                .get(http::header::IF_NONE_MATCH)
                .and_then(|value| value.to_str().ok()),
            Some("*")
        );
        assert_eq!(
            headers
                .get("x-amz-meta-lease-owner")
                .and_then(|value| value.to_str().ok()),
            Some("client-1")
        );
    }

    #[test]
    fn xml_body_headers_include_content_type_and_md5() {
        let headers = xml_body_headers(b"<Tagging/>").expect("headers should be valid");

        assert_eq!(
            headers
                .get(http::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/xml")
        );
        assert_eq!(
            headers
                .get("content-md5")
                .and_then(|value| value.to_str().ok()),
            Some("5MKq9Afjj8VFAV5vB64atA==")
        );
    }

    #[test]
    fn push_delete_object_rejects_oversized_batches() {
        let mut objects = Vec::new();
        for idx in 0..MAX_DELETE_OBJECTS_PER_REQUEST {
            push_delete_object(
                &mut objects,
                DeleteObjectIdentifier::new(format!("key-{idx}")).unwrap(),
            )
            .unwrap();
        }

        let err = push_delete_object(
            &mut objects,
            DeleteObjectIdentifier::new("one-too-many").unwrap(),
        )
        .expect_err("oversized delete batch must be rejected before send");

        match err {
            Error::InvalidConfig { message } => assert!(message.contains("at most 1000")),
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
        assert_eq!(objects.len(), MAX_DELETE_OBJECTS_PER_REQUEST);
    }

    #[cfg(feature = "multipart")]
    #[test]
    fn prepare_completed_parts_sorts_valid_parts() {
        let parts = prepare_completed_parts(vec![
            CompletedPart::new(2, "\"etag-2\"").unwrap(),
            CompletedPart::new(1, "\"etag-1\"").unwrap(),
        ])
        .expect("parts should be valid");

        assert_eq!(parts[0].part_number(), 1);
        assert_eq!(parts[1].part_number(), 2);
    }

    #[cfg(feature = "multipart")]
    #[test]
    fn completed_part_rejects_invalid_values() {
        assert!(CompletedPart::new(0, "etag").is_err());
        assert!(CompletedPart::new(1, " ").is_err());
        assert!(CompletedPart::new(1, " etag").is_err());
        assert!(CompletedPart::new(1, "etag").is_err());
        assert!(CompletedPart::new(1, "\"et ag\"").is_err());
        assert!(CompletedPart::new(1, "\"\"").is_err());
        assert!(CompletedPart::new(1, "\"bad\"etag\"").is_err());
    }

    #[cfg(feature = "multipart")]
    #[test]
    fn prepare_completed_parts_rejects_empty_or_duplicate_parts() {
        assert!(prepare_completed_parts(Vec::new()).is_err());
        assert!(
            prepare_completed_parts(vec![
                CompletedPart::new(1, "\"etag-1\"").unwrap(),
                CompletedPart::new(1, "\"etag-duplicate\"").unwrap(),
            ])
            .is_err()
        );
    }

    #[cfg(feature = "multipart")]
    #[test]
    fn push_completed_part_rejects_duplicate_and_oversized_batches() {
        let mut parts = Vec::new();
        push_completed_part(&mut parts, CompletedPart::new(1, "\"etag-1\"").unwrap()).unwrap();
        let err = push_completed_part(
            &mut parts,
            CompletedPart::new(1, "\"etag-duplicate\"").unwrap(),
        )
        .expect_err("duplicate completed part must be rejected before send");
        match err {
            Error::InvalidConfig { message } => assert!(message.contains("unique")),
            other => panic!("expected InvalidConfig, got {other:?}"),
        }

        let mut parts = Vec::new();
        for part_number in 1..=MAX_UPLOAD_PART_NUMBER {
            push_completed_part(
                &mut parts,
                CompletedPart::new(part_number, format!("\"etag-{part_number}\"")).unwrap(),
            )
            .unwrap();
        }
        let err = push_completed_part(
            &mut parts,
            CompletedPart::new(MAX_UPLOAD_PART_NUMBER, "\"duplicate\"").unwrap(),
        )
        .expect_err("oversized completed part batch must be rejected before send");
        match err {
            Error::InvalidConfig { message } => assert!(message.contains("at most 10000")),
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
        assert_eq!(parts.len(), MAX_UPLOAD_PART_NUMBER as usize);
    }

    #[cfg(feature = "multipart")]
    #[test]
    fn validate_upload_id_rejects_outer_whitespace() {
        assert!(validate_upload_id("").is_err());
        assert!(validate_upload_id(" upload-id").is_err());
        assert!(validate_upload_id("upload-id ").is_err());
        assert!(validate_upload_id("upload id").is_err());
        assert!(validate_upload_id("upload\nid").is_err());
        assert!(validate_upload_id("upload-id").is_ok());
    }

    #[test]
    fn validate_query_token_rejects_ambiguous_values() {
        assert!(validate_query_token("continuation_token", "").is_err());
        assert!(validate_query_token("continuation_token", " token").is_err());
        assert!(validate_query_token("continuation_token", "token ").is_err());
        assert!(validate_query_token("continuation_token", "tok en").is_err());
        assert!(validate_query_token("continuation_token", "tok\nen").is_err());
        assert!(validate_query_token("continuation_token", "opaque/token+id=").is_ok());
    }

    #[test]
    fn validate_query_value_rejects_ambiguous_values() {
        validate_query_value("prefix", "photos/2026 ").unwrap();

        for value in ["", "line\nbreak", "bad\u{7f}"] {
            let err = validate_query_value("prefix", value).expect_err("expected invalid query");
            match err {
                Error::InvalidConfig { .. } => {}
                other => panic!("expected InvalidConfig, got {other:?}"),
            }
        }
    }

    #[test]
    fn validate_subresource_rejects_blank_values() {
        assert!(validate_subresource("versioning").is_ok());
        assert!(validate_subresource("").is_err());
        assert!(validate_subresource("   ").is_err());
        assert!(validate_subresource(" versioning").is_err());
        assert!(validate_subresource("versioning ").is_err());
        assert!(validate_subresource("bucket versioning").is_err());
        assert!(validate_subresource("versioning\n").is_err());
    }

    #[test]
    fn apply_metadata_headers_writes_expected_headers() {
        let mut headers = HeaderMap::new();
        apply_metadata_headers(
            &mut headers,
            vec![
                ("owner".to_string(), "alice".to_string()),
                ("trace-id".to_string(), "abc-123".to_string()),
            ],
        )
        .expect("metadata should map to headers");

        assert_eq!(
            headers
                .get("x-amz-meta-owner")
                .and_then(|v| v.to_str().ok()),
            Some("alice")
        );
        assert_eq!(
            headers
                .get("x-amz-meta-trace-id")
                .and_then(|v| v.to_str().ok()),
            Some("abc-123")
        );
    }

    #[test]
    fn apply_metadata_headers_rejects_duplicate_keys_after_normalization() {
        let mut headers = HeaderMap::new();
        let err = apply_metadata_headers(
            &mut headers,
            vec![
                ("Owner".to_string(), "alice".to_string()),
                ("owner".to_string(), "bob".to_string()),
            ],
        )
        .expect_err("metadata keys normalize to the same header");

        match err {
            Error::InvalidConfig { message } => assert!(message.contains("unique")),
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
        assert!(headers.is_empty());
    }

    #[test]
    fn metadata_values_reject_outer_whitespace_but_allow_empty() {
        let mut metadata = Vec::new();
        push_metadata(&mut metadata, "empty", "").expect("empty metadata values are valid");
        assert_eq!(metadata, vec![("empty".to_string(), String::new())]);

        let err = push_metadata(&mut metadata, "bad", " value")
            .expect_err("metadata value with outer whitespace must be rejected");
        match err {
            Error::InvalidConfig { message } => assert!(message.contains("metadata values")),
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
    }

    #[test]
    fn apply_metadata_headers_rejects_existing_header_collision() {
        let mut headers = HeaderMap::new();
        headers.insert("x-amz-meta-owner", HeaderValue::from_static("alice"));
        let err =
            apply_metadata_headers(&mut headers, vec![("owner".to_string(), "bob".to_string())])
                .expect_err("metadata must not overwrite existing headers");

        match err {
            Error::InvalidConfig { message } => assert!(message.contains("unique")),
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
    }

    #[test]
    fn copy_metadata_headers_only_replace_when_requested_or_overridden() {
        let mut headers = HeaderMap::new();
        apply_copy_metadata_headers(&mut headers, false, None, Vec::new())
            .expect("empty copy metadata should be valid");
        assert!(!headers.contains_key("x-amz-metadata-directive"));

        let mut headers = HeaderMap::new();
        apply_copy_metadata_headers(&mut headers, true, None, Vec::new())
            .expect("explicit metadata replacement should be valid");
        assert_eq!(
            headers
                .get("x-amz-metadata-directive")
                .and_then(|v| v.to_str().ok()),
            Some("REPLACE")
        );

        let mut headers = HeaderMap::new();
        apply_copy_metadata_headers(
            &mut headers,
            false,
            Some("text/plain".to_string()),
            Vec::new(),
        )
        .expect("content type override should be valid");
        assert_eq!(
            headers
                .get("x-amz-metadata-directive")
                .and_then(|v| v.to_str().ok()),
            Some("REPLACE")
        );

        let mut headers = HeaderMap::new();
        apply_copy_metadata_headers(
            &mut headers,
            false,
            None,
            vec![("color".to_string(), "blue".to_string())],
        )
        .expect("metadata override should be valid");
        assert_eq!(
            headers
                .get("x-amz-metadata-directive")
                .and_then(|v| v.to_str().ok()),
            Some("REPLACE")
        );
    }

    #[test]
    fn parse_xml_or_service_error_maps_request_id_only_error_payload() {
        let body = "<Error><RequestId>req-only</RequestId></Error>";
        let err = parse_xml_or_service_error::<()>(
            http::StatusCode::BAD_REQUEST,
            &http::HeaderMap::new(),
            body,
            |_| Err(Error::decode("failed to parse expected xml", None)),
        )
        .expect_err("request-id-only payload should map to API error");

        match err {
            Error::Api {
                status, request_id, ..
            } => {
                assert_eq!(status, http::StatusCode::BAD_REQUEST);
                assert_eq!(request_id.as_deref(), Some("req-only"));
            }
            other => panic!("expected Api error, got {other:?}"),
        }
    }
}
