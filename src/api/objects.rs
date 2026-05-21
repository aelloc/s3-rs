//! Async object operations.

use std::time::Duration;

use bytes::Bytes;
use futures_core::Stream;
use http::{HeaderMap, HeaderValue, Method, StatusCode};

#[cfg(test)]
use super::common::parse_xml_or_service_error;
use super::common::{
    ByteRange, apply_copy_metadata_headers, apply_metadata_headers, insert_header,
    insert_optional_header, parse_async_xml_response, validate_content_length_matches_body,
    validate_max_keys, validate_query_token, validate_query_value,
};
#[cfg(feature = "multipart")]
use super::common::{
    prepare_completed_parts, validate_max_parts, validate_part_number_marker, validate_upload_id,
    validate_upload_part_number,
};

use crate::{
    client::Client,
    error::{Error, Result},
    transport::async_transport::{AsyncBody, boxed_byte_stream, response_error},
    types::{
        CopyObjectOutput, DeleteObjectIdentifier, DeleteObjectOutput, DeleteObjectsOutput,
        GetObjectOutput, HeadObjectOutput, ListObjectsV2Output, PresignedRequest, PutObjectOutput,
    },
};

const MAX_ERROR_RESPONSE_BODY_BYTES: usize = 256 * 1024;

#[cfg(feature = "multipart")]
use crate::types::{
    AbortMultipartUploadOutput, CompleteMultipartUploadOutput, CompletedPart,
    CreateMultipartUploadOutput, ListPartsOutput, UploadPartCopyOutput, UploadPartOutput,
};

/// Object operations service.
///
/// Created by [`Client::objects`](crate::Client::objects).
///
/// Start here for common object flows:
///
/// - [`get`](crate::api::ObjectsService::get) to download object bytes
/// - [`put`](crate::api::ObjectsService::put) to upload bytes or streams
/// - [`list_v2`](crate::api::ObjectsService::list_v2) to list object keys
/// - [`presign_get`](crate::api::ObjectsService::presign_get) to build a presigned download URL
#[derive(Clone)]
pub struct ObjectsService {
    client: Client,
}

impl ObjectsService {
    pub(crate) fn new(client: Client) -> Self {
        Self { client }
    }

    /// Starts a request to GET an object.
    pub fn get(&self, bucket: impl Into<String>, key: impl Into<String>) -> GetObjectRequest {
        GetObjectRequest {
            client: self.client.clone(),
            bucket: bucket.into(),
            key: key.into(),
            range: None,
            if_match: None,
            if_none_match: None,
            if_modified_since: None,
            if_unmodified_since: None,
        }
    }

    /// Starts a request to HEAD an object.
    pub fn head(&self, bucket: impl Into<String>, key: impl Into<String>) -> HeadObjectRequest {
        HeadObjectRequest {
            client: self.client.clone(),
            bucket: bucket.into(),
            key: key.into(),
        }
    }

    /// Starts a request to PUT an object.
    pub fn put(&self, bucket: impl Into<String>, key: impl Into<String>) -> PutObjectRequest {
        PutObjectRequest {
            client: self.client.clone(),
            bucket: bucket.into(),
            key: key.into(),
            content_type: None,
            cache_control: None,
            content_disposition: None,
            content_encoding: None,
            content_language: None,
            expires: None,
            content_length: None,
            #[cfg(feature = "checksums")]
            checksum: None,
            metadata: Vec::new(),
            body: AsyncBody::Empty,
        }
    }

    /// Starts a request to DELETE an object.
    pub fn delete(&self, bucket: impl Into<String>, key: impl Into<String>) -> DeleteObjectRequest {
        DeleteObjectRequest {
            client: self.client.clone(),
            bucket: bucket.into(),
            key: key.into(),
        }
    }

    /// Starts a request to DELETE multiple objects.
    pub fn delete_objects(&self, bucket: impl Into<String>) -> DeleteObjectsRequest {
        DeleteObjectsRequest {
            client: self.client.clone(),
            bucket: bucket.into(),
            objects: Vec::new(),
            quiet: false,
        }
    }

    /// Starts a request to copy an object.
    pub fn copy(
        &self,
        source_bucket: impl Into<String>,
        source_key: impl Into<String>,
        destination_bucket: impl Into<String>,
        destination_key: impl Into<String>,
    ) -> CopyObjectRequest {
        CopyObjectRequest {
            client: self.client.clone(),
            source_bucket: source_bucket.into(),
            source_key: source_key.into(),
            source_version_id: None,
            destination_bucket: destination_bucket.into(),
            destination_key: destination_key.into(),
            replace_metadata: false,
            metadata: Vec::new(),
            content_type: None,
        }
    }

    #[cfg(feature = "multipart")]
    /// Starts a multipart upload.
    pub fn create_multipart_upload(
        &self,
        bucket: impl Into<String>,
        key: impl Into<String>,
    ) -> CreateMultipartUploadRequest {
        CreateMultipartUploadRequest {
            client: self.client.clone(),
            bucket: bucket.into(),
            key: key.into(),
            content_type: None,
            metadata: Vec::new(),
        }
    }

    #[cfg(feature = "multipart")]
    /// Starts a request to upload a multipart part.
    pub fn upload_part(
        &self,
        bucket: impl Into<String>,
        key: impl Into<String>,
        upload_id: impl Into<String>,
        part_number: u32,
    ) -> UploadPartRequest {
        UploadPartRequest {
            client: self.client.clone(),
            bucket: bucket.into(),
            key: key.into(),
            upload_id: upload_id.into(),
            part_number,
            body: AsyncBody::Empty,
        }
    }

    #[cfg(feature = "multipart")]
    /// Starts a request to copy data into a multipart part.
    pub fn upload_part_copy(
        &self,
        source_bucket: impl Into<String>,
        source_key: impl Into<String>,
        destination_bucket: impl Into<String>,
        destination_key: impl Into<String>,
        upload_id: impl Into<String>,
        part_number: u32,
    ) -> UploadPartCopyRequest {
        UploadPartCopyRequest {
            client: self.client.clone(),
            source_bucket: source_bucket.into(),
            source_key: source_key.into(),
            source_version_id: None,
            destination_bucket: destination_bucket.into(),
            destination_key: destination_key.into(),
            upload_id: upload_id.into(),
            part_number,
            copy_source_range: None,
        }
    }

    #[cfg(feature = "multipart")]
    /// Starts a request to complete a multipart upload.
    pub fn complete_multipart_upload(
        &self,
        bucket: impl Into<String>,
        key: impl Into<String>,
        upload_id: impl Into<String>,
    ) -> CompleteMultipartUploadRequest {
        CompleteMultipartUploadRequest {
            client: self.client.clone(),
            bucket: bucket.into(),
            key: key.into(),
            upload_id: upload_id.into(),
            parts: Vec::new(),
        }
    }

    #[cfg(feature = "multipart")]
    /// Starts a request to abort a multipart upload.
    pub fn abort_multipart_upload(
        &self,
        bucket: impl Into<String>,
        key: impl Into<String>,
        upload_id: impl Into<String>,
    ) -> AbortMultipartUploadRequest {
        AbortMultipartUploadRequest {
            client: self.client.clone(),
            bucket: bucket.into(),
            key: key.into(),
            upload_id: upload_id.into(),
        }
    }

    #[cfg(feature = "multipart")]
    /// Starts a request to list multipart parts.
    pub fn list_parts(
        &self,
        bucket: impl Into<String>,
        key: impl Into<String>,
        upload_id: impl Into<String>,
    ) -> ListPartsRequest {
        ListPartsRequest {
            client: self.client.clone(),
            bucket: bucket.into(),
            key: key.into(),
            upload_id: upload_id.into(),
            max_parts: None,
            part_number_marker: None,
        }
    }

    /// Starts a ListObjectsV2 request.
    pub fn list_v2(&self, bucket: impl Into<String>) -> ListObjectsV2Request {
        ListObjectsV2Request {
            client: self.client.clone(),
            bucket: bucket.into(),
            prefix: None,
            delimiter: None,
            continuation_token: None,
            start_after: None,
            max_keys: None,
        }
    }

    /// Starts a generic presign request builder.
    pub fn presign(
        &self,
        method: Method,
        bucket: impl Into<String>,
        key: impl Into<String>,
    ) -> PresignObjectRequest {
        PresignObjectRequest {
            client: self.client.clone(),
            method,
            bucket: bucket.into(),
            key: key.into(),
            expires_in: Duration::from_secs(900),
            query_params: Vec::new(),
            headers: HeaderMap::new(),
            metadata: Vec::new(),
        }
    }

    /// Starts a presigned GET request builder.
    pub fn presign_get(
        &self,
        bucket: impl Into<String>,
        key: impl Into<String>,
    ) -> PresignGetObjectRequest {
        PresignGetObjectRequest {
            client: self.client.clone(),
            bucket: bucket.into(),
            key: key.into(),
            expires_in: Duration::from_secs(900),
            query_params: Vec::new(),
            headers: HeaderMap::new(),
            metadata: Vec::new(),
        }
    }

    /// Starts a presigned PUT request builder.
    pub fn presign_put(
        &self,
        bucket: impl Into<String>,
        key: impl Into<String>,
    ) -> PresignPutObjectRequest {
        PresignPutObjectRequest {
            client: self.client.clone(),
            bucket: bucket.into(),
            key: key.into(),
            expires_in: Duration::from_secs(900),
            query_params: Vec::new(),
            headers: HeaderMap::new(),
            metadata: Vec::new(),
        }
    }

    /// Starts a presigned HEAD request builder.
    pub fn presign_head(
        &self,
        bucket: impl Into<String>,
        key: impl Into<String>,
    ) -> PresignHeadObjectRequest {
        PresignHeadObjectRequest {
            client: self.client.clone(),
            bucket: bucket.into(),
            key: key.into(),
            expires_in: Duration::from_secs(900),
            query_params: Vec::new(),
            headers: HeaderMap::new(),
        }
    }

    /// Starts a presigned DELETE request builder.
    pub fn presign_delete(
        &self,
        bucket: impl Into<String>,
        key: impl Into<String>,
    ) -> PresignDeleteObjectRequest {
        PresignDeleteObjectRequest {
            client: self.client.clone(),
            bucket: bucket.into(),
            key: key.into(),
            expires_in: Duration::from_secs(900),
            query_params: Vec::new(),
            headers: HeaderMap::new(),
        }
    }
}

/// Request builder for fetching an object.
///
/// Created by [`ObjectsService::get`](crate::api::ObjectsService::get).
///
/// # Example
///
/// ```no_run
/// # async fn demo() -> Result<(), s3::Error> {
/// use s3::{Auth, Client};
///
/// let client = Client::builder("https://s3.example.com")?
///     .region("us-east-1")
///     .auth(Auth::from_env()?)
///     .build()?;
///
/// let output = client
///     .objects()
///     .get("my-bucket", "logs/app.log")
///     .send()
///     .await?;
/// let bytes = output.bytes().await?;
/// # let _ = bytes;
/// # Ok(())
/// # }
/// ```
pub struct GetObjectRequest {
    client: Client,
    bucket: String,
    key: String,
    range: Option<ByteRange>,
    if_match: Option<String>,
    if_none_match: Option<String>,
    if_modified_since: Option<String>,
    if_unmodified_since: Option<String>,
}

impl GetObjectRequest {
    /// Sets an inclusive byte range.
    pub fn range_bytes(mut self, start: u64, end_inclusive: u64) -> Self {
        self.range = Some(ByteRange::new(start, end_inclusive));
        self
    }

    /// Adds an If-Match condition.
    pub fn if_match(mut self, value: impl Into<String>) -> Self {
        self.if_match = Some(value.into());
        self
    }

    /// Adds an If-None-Match condition.
    pub fn if_none_match(mut self, value: impl Into<String>) -> Self {
        self.if_none_match = Some(value.into());
        self
    }

    /// Adds an If-Modified-Since condition.
    pub fn if_modified_since(mut self, value: impl Into<String>) -> Self {
        self.if_modified_since = Some(value.into());
        self
    }

    /// Adds an If-Unmodified-Since condition.
    pub fn if_unmodified_since(mut self, value: impl Into<String>) -> Self {
        self.if_unmodified_since = Some(value.into());
        self
    }

    /// Sends the request.
    pub async fn send(self) -> Result<GetObjectOutput> {
        let mut headers = HeaderMap::new();
        if let Some(range) = self.range {
            headers.insert(
                http::header::RANGE,
                range.header_value("invalid Range header")?,
            );
        }
        insert_optional_header(
            &mut headers,
            http::header::IF_MATCH,
            self.if_match,
            "invalid If-Match header",
        )?;
        insert_optional_header(
            &mut headers,
            http::header::IF_NONE_MATCH,
            self.if_none_match,
            "invalid If-None-Match header",
        )?;
        insert_optional_header(
            &mut headers,
            http::header::IF_MODIFIED_SINCE,
            self.if_modified_since,
            "invalid If-Modified-Since header",
        )?;
        insert_optional_header(
            &mut headers,
            http::header::IF_UNMODIFIED_SINCE,
            self.if_unmodified_since,
            "invalid If-Unmodified-Since header",
        )?;

        let resp = self
            .client
            .execute_stream(
                Method::GET,
                Some(&self.bucket),
                Some(&self.key),
                Vec::new(),
                headers,
                AsyncBody::Empty,
            )
            .await?;

        if !resp.status().is_success() {
            let resp = resp
                .into_response_limited(MAX_ERROR_RESPONSE_BODY_BYTES)
                .await
                .map_err(|e| Error::transport("failed to read response body", Some(Box::new(e))))?;
            return Err(response_error(
                crate::transport::async_transport::AsyncResponse::from_reqx(resp),
            ));
        }

        let etag = crate::util::headers::header_string(resp.headers(), http::header::ETAG);
        let content_length =
            crate::util::headers::header_u64(resp.headers(), http::header::CONTENT_LENGTH);
        let content_type =
            crate::util::headers::header_string(resp.headers(), http::header::CONTENT_TYPE);

        let stream = futures_util::stream::try_unfold(resp, |mut resp| async move {
            use tokio::io::AsyncReadExt as _;

            let mut chunk = vec![0; 8192];
            let read = resp
                .read(&mut chunk)
                .await
                .map_err(|e| Error::transport("body stream error", Some(Box::new(e))))?;
            if read == 0 {
                return Ok(None);
            }
            chunk.truncate(read);
            Ok(Some((Bytes::from(chunk), resp)))
        });

        Ok(GetObjectOutput {
            body: Box::pin(stream),
            etag,
            content_length,
            content_type,
        })
    }
}

/// Request builder for fetching object metadata via HEAD.
pub struct HeadObjectRequest {
    client: Client,
    bucket: String,
    key: String,
}

impl HeadObjectRequest {
    /// Sends the request.
    pub async fn send(self) -> Result<HeadObjectOutput> {
        let resp = self
            .client
            .execute(
                Method::HEAD,
                Some(&self.bucket),
                Some(&self.key),
                Vec::new(),
                HeaderMap::new(),
                AsyncBody::Empty,
            )
            .await?;

        if !resp.status().is_success() {
            return Err(response_error(resp));
        }

        Ok(HeadObjectOutput {
            etag: crate::util::headers::header_string(resp.headers(), http::header::ETAG),
            content_length: crate::util::headers::header_u64(
                resp.headers(),
                http::header::CONTENT_LENGTH,
            ),
            content_type: crate::util::headers::header_string(
                resp.headers(),
                http::header::CONTENT_TYPE,
            ),
        })
    }
}

/// Request builder for uploading an object.
///
/// Created by [`ObjectsService::put`](crate::api::ObjectsService::put).
///
/// # Example
///
/// ```no_run
/// # async fn demo() -> Result<(), s3::Error> {
/// use s3::{Auth, Client};
///
/// let client = Client::builder("https://s3.example.com")?
///     .region("us-east-1")
///     .auth(Auth::from_env()?)
///     .build()?;
///
/// let output = client
///     .objects()
///     .put("my-bucket", "notes/hello.txt")
///     .content_type("text/plain; charset=utf-8")
///     .body_bytes("hello from s3-rs")
///     .send()
///     .await?;
/// # let _ = output;
/// # Ok(())
/// # }
/// ```
pub struct PutObjectRequest {
    client: Client,
    bucket: String,
    key: String,
    content_type: Option<String>,
    cache_control: Option<String>,
    content_disposition: Option<String>,
    content_encoding: Option<String>,
    content_language: Option<String>,
    expires: Option<String>,
    content_length: Option<u64>,
    #[cfg(feature = "checksums")]
    checksum: Option<crate::types::Checksum>,
    metadata: Vec<(String, String)>,
    body: AsyncBody,
}

impl PutObjectRequest {
    /// Sets the Content-Type header.
    pub fn content_type(mut self, value: impl Into<String>) -> Self {
        self.content_type = Some(value.into());
        self
    }

    /// Sets the Cache-Control header.
    pub fn cache_control(mut self, value: impl Into<String>) -> Self {
        self.cache_control = Some(value.into());
        self
    }

    /// Sets the Content-Disposition header.
    pub fn content_disposition(mut self, value: impl Into<String>) -> Self {
        self.content_disposition = Some(value.into());
        self
    }

    /// Sets the Content-Encoding header.
    pub fn content_encoding(mut self, value: impl Into<String>) -> Self {
        self.content_encoding = Some(value.into());
        self
    }

    /// Sets the Content-Language header.
    pub fn content_language(mut self, value: impl Into<String>) -> Self {
        self.content_language = Some(value.into());
        self
    }

    /// Sets the Expires header.
    pub fn expires(mut self, value: impl Into<String>) -> Self {
        self.expires = Some(value.into());
        self
    }

    /// Sets the content length (required for streaming bodies).
    pub fn content_length(mut self, value: u64) -> Self {
        self.content_length = Some(value);
        self
    }

    /// Adds a user metadata entry.
    pub fn metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.push((key.into(), value.into()));
        self
    }

    #[cfg(feature = "checksums")]
    /// Sets a checksum to be sent with the upload.
    pub fn checksum(mut self, checksum: crate::types::Checksum) -> Self {
        self.checksum = Some(checksum);
        self
    }

    /// Sets the request body from bytes.
    pub fn body_bytes(mut self, body: impl Into<Bytes>) -> Self {
        self.body = AsyncBody::Bytes(body.into());
        self
    }

    /// Sets the request body from a byte stream.
    pub fn body_stream<S, E>(mut self, stream: S) -> Self
    where
        S: Stream<Item = std::result::Result<Bytes, E>> + Send + 'static,
        E: std::error::Error + Send + Sync + 'static,
    {
        self.body = AsyncBody::Stream {
            stream: boxed_byte_stream(stream),
            content_length: None,
        };
        self
    }

    /// Sets a streaming body with a known content length.
    pub fn body_stream_sized<S, E>(mut self, stream: S, content_length: u64) -> Self
    where
        S: Stream<Item = std::result::Result<Bytes, E>> + Send + 'static,
        E: std::error::Error + Send + Sync + 'static,
    {
        self.content_length = Some(content_length);
        self.body_stream(stream)
    }

    /// Sends the request.
    pub async fn send(self) -> Result<PutObjectOutput> {
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

        apply_metadata_headers(&mut headers, self.metadata)?;

        #[cfg(feature = "checksums")]
        if let Some(checksum) = self.checksum {
            checksum.apply(&mut headers)?;
        }

        let body = match self.body {
            AsyncBody::Empty => {
                validate_content_length_matches_body(self.content_length, 0, "put_object")?;
                AsyncBody::Bytes(Bytes::new())
            }
            AsyncBody::Bytes(bytes) => {
                validate_content_length_matches_body(
                    self.content_length,
                    bytes.len(),
                    "put_object",
                )?;
                AsyncBody::Bytes(bytes)
            }
            AsyncBody::Stream { stream, .. } => {
                let content_length = self.content_length.ok_or_else(|| {
                    Error::invalid_config("streaming put requires content_length")
                })?;
                AsyncBody::Stream {
                    stream,
                    content_length: Some(content_length),
                }
            }
        };

        let resp = self
            .client
            .execute(
                Method::PUT,
                Some(&self.bucket),
                Some(&self.key),
                Vec::new(),
                headers,
                body,
            )
            .await?;

        if !resp.status().is_success() {
            return Err(response_error(resp));
        }

        Ok(PutObjectOutput {
            etag: crate::util::headers::header_string(resp.headers(), http::header::ETAG),
        })
    }
}

/// Request builder for deleting a single object.
pub struct DeleteObjectRequest {
    client: Client,
    bucket: String,
    key: String,
}

impl DeleteObjectRequest {
    /// Sends the request.
    pub async fn send(self) -> Result<DeleteObjectOutput> {
        let resp = self
            .client
            .execute(
                Method::DELETE,
                Some(&self.bucket),
                Some(&self.key),
                Vec::new(),
                HeaderMap::new(),
                AsyncBody::Empty,
            )
            .await?;

        if resp.status() == StatusCode::NO_CONTENT || resp.status().is_success() {
            return Ok(DeleteObjectOutput);
        }

        Err(response_error(resp))
    }
}

/// Request builder for deleting multiple objects.
pub struct DeleteObjectsRequest {
    client: Client,
    bucket: String,
    objects: Vec<DeleteObjectIdentifier>,
    quiet: bool,
}

impl DeleteObjectsRequest {
    /// Adds an object key to delete.
    pub fn object(mut self, key: impl Into<String>) -> Self {
        self.objects.push(DeleteObjectIdentifier::new(key));
        self
    }

    /// Adds an object key and version id to delete.
    pub fn object_with_version(
        mut self,
        key: impl Into<String>,
        version_id: impl Into<String>,
    ) -> Self {
        self.objects
            .push(DeleteObjectIdentifier::new(key).with_version_id(version_id));
        self
    }

    /// Adds multiple object keys to delete.
    pub fn objects<I, S>(mut self, iter: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.objects
            .extend(iter.into_iter().map(DeleteObjectIdentifier::new));
        self
    }

    /// Toggles quiet response mode.
    pub fn quiet(mut self, quiet: bool) -> Self {
        self.quiet = quiet;
        self
    }

    /// Sends the request.
    pub async fn send(self) -> Result<DeleteObjectsOutput> {
        let body = crate::util::xml::encode_delete_objects(&self.objects, self.quiet)?;
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/xml"),
        );
        let content_md5 = crate::util::md5::content_md5_header_value(body.as_ref())?;
        headers.insert(
            http::header::HeaderName::from_static("content-md5"),
            content_md5,
        );

        let resp = self
            .client
            .execute(
                Method::POST,
                Some(&self.bucket),
                None,
                vec![("delete".to_string(), String::new())],
                headers,
                AsyncBody::Bytes(body),
            )
            .await?;

        if !resp.status().is_success() {
            return Err(response_error(resp));
        }

        parse_async_xml_response(resp, crate::util::xml::parse_delete_objects)
    }
}

/// Request builder for copying an object.
pub struct CopyObjectRequest {
    client: Client,
    source_bucket: String,
    source_key: String,
    source_version_id: Option<String>,
    destination_bucket: String,
    destination_key: String,
    replace_metadata: bool,
    metadata: Vec<(String, String)>,
    content_type: Option<String>,
}

impl CopyObjectRequest {
    /// Sets a source version id to copy.
    pub fn source_version_id(mut self, version_id: impl Into<String>) -> Self {
        self.source_version_id = Some(version_id.into());
        self
    }

    /// Replaces metadata on the destination object.
    pub fn replace_metadata(mut self) -> Self {
        self.replace_metadata = true;
        self
    }

    /// Adds a user metadata entry.
    pub fn metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.push((key.into(), value.into()));
        self
    }

    /// Sets the Content-Type for the destination object.
    pub fn content_type(mut self, value: impl Into<String>) -> Self {
        self.content_type = Some(value.into());
        self
    }

    /// Sends the request.
    pub async fn send(self) -> Result<CopyObjectOutput> {
        let mut headers = HeaderMap::new();

        let copy_source = crate::util::headers::copy_source_header_value(
            &self.source_bucket,
            &self.source_key,
            self.source_version_id.as_deref(),
        )?;
        insert_header(
            &mut headers,
            "x-amz-copy-source",
            copy_source,
            "invalid x-amz-copy-source header",
        )?;

        apply_copy_metadata_headers(
            &mut headers,
            self.replace_metadata,
            self.content_type,
            self.metadata,
        )?;

        let resp = self
            .client
            .execute(
                Method::PUT,
                Some(&self.destination_bucket),
                Some(&self.destination_key),
                Vec::new(),
                headers,
                AsyncBody::Empty,
            )
            .await?;

        if !resp.status().is_success() {
            return Err(response_error(resp));
        }

        parse_async_xml_response(resp, crate::util::xml::parse_copy_object)
    }
}

#[cfg(feature = "multipart")]
/// Request builder for initiating a multipart upload.
pub struct CreateMultipartUploadRequest {
    client: Client,
    bucket: String,
    key: String,
    content_type: Option<String>,
    metadata: Vec<(String, String)>,
}

#[cfg(feature = "multipart")]
impl CreateMultipartUploadRequest {
    /// Sets the Content-Type header.
    pub fn content_type(mut self, value: impl Into<String>) -> Self {
        self.content_type = Some(value.into());
        self
    }

    /// Adds a user metadata entry.
    pub fn metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.push((key.into(), value.into()));
        self
    }

    /// Sends the request.
    pub async fn send(self) -> Result<CreateMultipartUploadOutput> {
        let mut headers = HeaderMap::new();
        insert_optional_header(
            &mut headers,
            http::header::CONTENT_TYPE,
            self.content_type,
            "invalid Content-Type header",
        )?;

        apply_metadata_headers(&mut headers, self.metadata)?;

        let resp = self
            .client
            .execute(
                Method::POST,
                Some(&self.bucket),
                Some(&self.key),
                vec![("uploads".to_string(), String::new())],
                headers,
                AsyncBody::Empty,
            )
            .await?;

        if !resp.status().is_success() {
            return Err(response_error(resp));
        }

        parse_async_xml_response(resp, crate::util::xml::parse_create_multipart_upload)
    }
}

#[cfg(feature = "multipart")]
/// Request builder for uploading a multipart part.
pub struct UploadPartRequest {
    client: Client,
    bucket: String,
    key: String,
    upload_id: String,
    part_number: u32,
    body: AsyncBody,
}

#[cfg(feature = "multipart")]
impl UploadPartRequest {
    /// Sets the request body from bytes.
    pub fn body_bytes(mut self, body: impl Into<Bytes>) -> Self {
        self.body = AsyncBody::Bytes(body.into());
        self
    }

    /// Sets a streaming request body with a known content length.
    pub fn body_stream_sized<S, E>(mut self, stream: S, content_length: u64) -> Self
    where
        S: Stream<Item = std::result::Result<Bytes, E>> + Send + 'static,
        E: std::error::Error + Send + Sync + 'static,
    {
        self.body = AsyncBody::Stream {
            stream: boxed_byte_stream(stream),
            content_length: Some(content_length),
        };
        self
    }

    /// Sends the request.
    pub async fn send(self) -> Result<UploadPartOutput> {
        validate_upload_part_body(&self.body)?;
        validate_upload_part_number(self.part_number)?;
        validate_upload_id(&self.upload_id)?;

        let query = vec![
            ("partNumber".to_string(), self.part_number.to_string()),
            ("uploadId".to_string(), self.upload_id),
        ];

        let resp = self
            .client
            .execute(
                Method::PUT,
                Some(&self.bucket),
                Some(&self.key),
                query,
                HeaderMap::new(),
                self.body,
            )
            .await?;

        if !resp.status().is_success() {
            return Err(response_error(resp));
        }

        Ok(UploadPartOutput {
            etag: crate::util::headers::header_string(resp.headers(), http::header::ETAG),
        })
    }
}

#[cfg(feature = "multipart")]
fn validate_upload_part_body(body: &AsyncBody) -> Result<()> {
    match body {
        AsyncBody::Empty => {
            return Err(Error::invalid_config("upload_part requires a request body"));
        }
        AsyncBody::Stream {
            content_length: None,
            ..
        } => {
            return Err(Error::invalid_config(
                "streaming upload_part requires content_length",
            ));
        }
        AsyncBody::Bytes(_)
        | AsyncBody::Stream {
            content_length: Some(_),
            ..
        } => {}
    }
    Ok(())
}

#[cfg(feature = "multipart")]
/// Request builder for uploading a copied multipart part.
pub struct UploadPartCopyRequest {
    client: Client,
    source_bucket: String,
    source_key: String,
    source_version_id: Option<String>,
    destination_bucket: String,
    destination_key: String,
    upload_id: String,
    part_number: u32,
    copy_source_range: Option<ByteRange>,
}

#[cfg(feature = "multipart")]
impl UploadPartCopyRequest {
    /// Sets the source version id to copy.
    pub fn source_version_id(mut self, version_id: impl Into<String>) -> Self {
        self.source_version_id = Some(version_id.into());
        self
    }

    /// Sets a byte range for the copy source.
    pub fn copy_source_range_bytes(mut self, start: u64, end_inclusive: u64) -> Self {
        self.copy_source_range = Some(ByteRange::new(start, end_inclusive));
        self
    }

    /// Sends the request.
    pub async fn send(self) -> Result<UploadPartCopyOutput> {
        validate_upload_part_number(self.part_number)?;
        validate_upload_id(&self.upload_id)?;

        let mut headers = HeaderMap::new();

        let copy_source = crate::util::headers::copy_source_header_value(
            &self.source_bucket,
            &self.source_key,
            self.source_version_id.as_deref(),
        )?;
        insert_header(
            &mut headers,
            "x-amz-copy-source",
            copy_source,
            "invalid x-amz-copy-source header",
        )?;

        if let Some(range) = self.copy_source_range {
            headers.insert(
                "x-amz-copy-source-range",
                range.header_value("invalid x-amz-copy-source-range header")?,
            );
        }

        let query = vec![
            ("partNumber".to_string(), self.part_number.to_string()),
            ("uploadId".to_string(), self.upload_id),
        ];

        let resp = self
            .client
            .execute(
                Method::PUT,
                Some(&self.destination_bucket),
                Some(&self.destination_key),
                query,
                headers,
                AsyncBody::Empty,
            )
            .await?;

        if !resp.status().is_success() {
            return Err(response_error(resp));
        }

        parse_async_xml_response(resp, crate::util::xml::parse_upload_part_copy)
    }
}

#[cfg(feature = "multipart")]
/// Request builder for completing a multipart upload.
pub struct CompleteMultipartUploadRequest {
    client: Client,
    bucket: String,
    key: String,
    upload_id: String,
    parts: Vec<CompletedPart>,
}

#[cfg(feature = "multipart")]
impl CompleteMultipartUploadRequest {
    /// Adds a completed part by number and etag.
    pub fn part(mut self, part_number: u32, etag: impl Into<String>) -> Self {
        self.parts.push(CompletedPart {
            part_number,
            etag: etag.into(),
        });
        self
    }

    /// Adds multiple completed parts.
    pub fn parts<I>(mut self, iter: I) -> Self
    where
        I: IntoIterator<Item = CompletedPart>,
    {
        self.parts.extend(iter);
        self
    }

    /// Sends the request.
    pub async fn send(self) -> Result<CompleteMultipartUploadOutput> {
        validate_upload_id(&self.upload_id)?;
        let parts = prepare_completed_parts(self.parts)?;
        let body = crate::util::xml::encode_complete_multipart_upload(&parts)?;
        let mut headers = HeaderMap::new();
        headers.insert(
            http::header::CONTENT_TYPE,
            HeaderValue::from_static("application/xml"),
        );

        let resp = self
            .client
            .execute(
                Method::POST,
                Some(&self.bucket),
                Some(&self.key),
                vec![("uploadId".to_string(), self.upload_id)],
                headers,
                AsyncBody::Bytes(body),
            )
            .await?;

        if !resp.status().is_success() {
            return Err(response_error(resp));
        }

        parse_async_xml_response(resp, crate::util::xml::parse_complete_multipart_upload)
    }
}

#[cfg(feature = "multipart")]
/// Request builder for aborting a multipart upload.
pub struct AbortMultipartUploadRequest {
    client: Client,
    bucket: String,
    key: String,
    upload_id: String,
}

#[cfg(feature = "multipart")]
impl AbortMultipartUploadRequest {
    /// Sends the request.
    pub async fn send(self) -> Result<AbortMultipartUploadOutput> {
        validate_upload_id(&self.upload_id)?;

        let resp = self
            .client
            .execute(
                Method::DELETE,
                Some(&self.bucket),
                Some(&self.key),
                vec![("uploadId".to_string(), self.upload_id)],
                HeaderMap::new(),
                AsyncBody::Empty,
            )
            .await?;

        if resp.status() == StatusCode::NO_CONTENT || resp.status().is_success() {
            return Ok(AbortMultipartUploadOutput);
        }

        Err(response_error(resp))
    }
}

#[cfg(feature = "multipart")]
/// Request builder for listing multipart parts.
pub struct ListPartsRequest {
    client: Client,
    bucket: String,
    key: String,
    upload_id: String,
    max_parts: Option<u32>,
    part_number_marker: Option<u32>,
}

#[cfg(feature = "multipart")]
impl ListPartsRequest {
    /// Sets the maximum number of parts to return.
    pub fn max_parts(mut self, value: u32) -> Self {
        self.max_parts = Some(value);
        self
    }

    /// Sets the part number marker for pagination.
    pub fn part_number_marker(mut self, value: u32) -> Self {
        self.part_number_marker = Some(value);
        self
    }

    /// Sends the request.
    pub async fn send(self) -> Result<ListPartsOutput> {
        validate_upload_id(&self.upload_id)?;

        let mut query = vec![("uploadId".to_string(), self.upload_id)];
        if let Some(v) = self.max_parts {
            validate_max_parts(v)?;
            query.push(("max-parts".to_string(), v.to_string()));
        }
        if let Some(v) = self.part_number_marker {
            validate_part_number_marker(v)?;
            query.push(("part-number-marker".to_string(), v.to_string()));
        }

        let resp = self
            .client
            .execute(
                Method::GET,
                Some(&self.bucket),
                Some(&self.key),
                query,
                HeaderMap::new(),
                AsyncBody::Empty,
            )
            .await?;

        if !resp.status().is_success() {
            return Err(response_error(resp));
        }

        parse_async_xml_response(resp, crate::util::xml::parse_list_parts)
    }
}

/// Request builder for ListObjectsV2.
///
/// Created by [`ObjectsService::list_v2`](crate::api::ObjectsService::list_v2).
///
/// # Example
///
/// ```no_run
/// # async fn demo() -> Result<(), s3::Error> {
/// use s3::{Auth, Client};
///
/// let client = Client::builder("https://s3.example.com")?
///     .region("us-east-1")
///     .auth(Auth::from_env()?)
///     .build()?;
///
/// let page = client
///     .objects()
///     .list_v2("my-bucket")
///     .prefix("logs/")
///     .max_keys(100)
///     .send()
///     .await?;
/// # let _ = page;
/// # Ok(())
/// # }
/// ```
pub struct ListObjectsV2Request {
    client: Client,
    bucket: String,
    prefix: Option<String>,
    delimiter: Option<String>,
    continuation_token: Option<String>,
    start_after: Option<String>,
    max_keys: Option<u32>,
}

impl ListObjectsV2Request {
    /// Filters by key prefix.
    pub fn prefix(mut self, value: impl Into<String>) -> Self {
        self.prefix = Some(value.into());
        self
    }

    /// Groups keys by delimiter.
    pub fn delimiter(mut self, value: impl Into<String>) -> Self {
        self.delimiter = Some(value.into());
        self
    }

    /// Sets the continuation token for pagination.
    pub fn continuation_token(mut self, value: impl Into<String>) -> Self {
        self.continuation_token = Some(value.into());
        self
    }

    /// Starts listing after the given key.
    pub fn start_after(mut self, value: impl Into<String>) -> Self {
        self.start_after = Some(value.into());
        self
    }

    /// Sets the maximum number of keys to return.
    pub fn max_keys(mut self, value: u32) -> Self {
        self.max_keys = Some(value);
        self
    }

    /// Converts this request into a pager.
    pub fn pager(self) -> ListObjectsV2Pager {
        ListObjectsV2Pager {
            client: self.client,
            bucket: self.bucket,
            prefix: self.prefix,
            delimiter: self.delimiter,
            continuation_token: self.continuation_token,
            start_after: self.start_after,
            max_keys: self.max_keys,
            done: false,
        }
    }

    /// Sends the request.
    pub async fn send(self) -> Result<ListObjectsV2Output> {
        let mut query = Vec::new();
        query.push(("list-type".to_string(), "2".to_string()));
        if let Some(v) = self.prefix {
            validate_query_value("prefix", &v)?;
            query.push(("prefix".to_string(), v));
        }
        if let Some(v) = self.delimiter {
            validate_query_value("delimiter", &v)?;
            query.push(("delimiter".to_string(), v));
        }
        if let Some(v) = self.continuation_token {
            validate_query_token("continuation_token", &v)?;
            query.push(("continuation-token".to_string(), v));
        }
        if let Some(v) = self.start_after {
            crate::util::url::validate_object_key(&v)?;
            query.push(("start-after".to_string(), v));
        }
        if let Some(v) = self.max_keys {
            validate_max_keys(v)?;
            query.push(("max-keys".to_string(), v.to_string()));
        }

        let resp = self
            .client
            .execute(
                Method::GET,
                Some(&self.bucket),
                None,
                query,
                HeaderMap::new(),
                AsyncBody::Empty,
            )
            .await?;

        if !resp.status().is_success() {
            return Err(response_error(resp));
        }

        parse_async_xml_response(resp, crate::util::xml::parse_list_objects_v2)
    }
}

/// Pager for ListObjectsV2 responses.
pub struct ListObjectsV2Pager {
    client: Client,
    bucket: String,
    prefix: Option<String>,
    delimiter: Option<String>,
    continuation_token: Option<String>,
    start_after: Option<String>,
    max_keys: Option<u32>,
    done: bool,
}

impl ListObjectsV2Pager {
    /// Fetches the next page, or returns None when complete.
    pub async fn next_page(&mut self) -> Result<Option<ListObjectsV2Output>> {
        if self.done {
            return Ok(None);
        }

        let start_after = if self.continuation_token.is_some() {
            None
        } else {
            self.start_after.clone()
        };

        let page = ListObjectsV2Request {
            client: self.client.clone(),
            bucket: self.bucket.clone(),
            prefix: self.prefix.clone(),
            delimiter: self.delimiter.clone(),
            continuation_token: self.continuation_token.clone(),
            start_after,
            max_keys: self.max_keys,
        }
        .send()
        .await?;

        self.continuation_token = page.next_continuation_token.clone();
        if !page.is_truncated {
            self.done = true;
        }

        Ok(Some(page))
    }
}

/// Request builder for presigned requests with a custom method.
pub struct PresignObjectRequest {
    client: Client,
    method: Method,
    bucket: String,
    key: String,
    expires_in: Duration,
    query_params: Vec<(String, String)>,
    headers: HeaderMap,
    metadata: Vec<(String, String)>,
}

impl PresignObjectRequest {
    /// Sets the expiry duration.
    pub fn expires_in(mut self, duration: Duration) -> Self {
        self.expires_in = duration;
        self
    }

    /// Adds a query parameter to the presigned URL.
    pub fn query_param(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.query_params.push((name.into(), value.into()));
        self
    }

    /// Adds an HTTP header to sign.
    pub fn header(mut self, name: http::header::HeaderName, value: HeaderValue) -> Self {
        self.headers.insert(name, value);
        self
    }

    /// Adds a user metadata entry to sign.
    pub fn metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.push((key.into(), value.into()));
        self
    }

    /// Builds the presigned request using static credentials.
    pub fn build(self) -> Result<PresignedRequest> {
        let mut headers = self.headers;
        apply_metadata_headers(&mut headers, self.metadata)?;

        self.client.presign(
            self.method,
            &self.bucket,
            &self.key,
            self.expires_in,
            self.query_params,
            headers,
        )
    }

    /// Builds the presigned request using the current credential provider.
    pub async fn build_async(self) -> Result<PresignedRequest> {
        let mut headers = self.headers;
        apply_metadata_headers(&mut headers, self.metadata)?;

        self.client
            .presign_async(
                self.method,
                &self.bucket,
                &self.key,
                self.expires_in,
                self.query_params,
                headers,
            )
            .await
    }
}

/// Request builder for presigned GET requests.
///
/// Created by [`ObjectsService::presign_get`](crate::api::ObjectsService::presign_get).
///
/// # Example
///
/// ```no_run
/// # async fn demo() -> Result<(), s3::Error> {
/// use std::time::Duration;
///
/// use s3::{Auth, Client};
///
/// let client = Client::builder("https://s3.example.com")?
///     .region("us-east-1")
///     .auth(Auth::from_env()?)
///     .build()?;
///
/// let presigned = client
///     .objects()
///     .presign_get("my-bucket", "reports/q1.csv")
///     .expires_in(Duration::from_secs(300))
///     .build()?;
/// # let _ = presigned;
/// # Ok(())
/// # }
/// ```
pub struct PresignGetObjectRequest {
    client: Client,
    bucket: String,
    key: String,
    expires_in: Duration,
    query_params: Vec<(String, String)>,
    headers: HeaderMap,
    metadata: Vec<(String, String)>,
}

impl PresignGetObjectRequest {
    /// Sets the expiry duration.
    pub fn expires_in(mut self, duration: Duration) -> Self {
        self.expires_in = duration;
        self
    }

    /// Adds a query parameter to the presigned URL.
    pub fn query_param(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.query_params.push((name.into(), value.into()));
        self
    }

    /// Adds an HTTP header to sign.
    pub fn header(mut self, name: http::header::HeaderName, value: HeaderValue) -> Self {
        self.headers.insert(name, value);
        self
    }

    /// Adds a user metadata entry to sign.
    pub fn metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.push((key.into(), value.into()));
        self
    }

    /// Builds the presigned request using static credentials.
    pub fn build(self) -> Result<PresignedRequest> {
        let mut headers = self.headers;
        apply_metadata_headers(&mut headers, self.metadata)?;

        self.client.presign(
            Method::GET,
            &self.bucket,
            &self.key,
            self.expires_in,
            self.query_params,
            headers,
        )
    }

    /// Builds the presigned request using the current credential provider.
    pub async fn build_async(self) -> Result<PresignedRequest> {
        let mut headers = self.headers;
        apply_metadata_headers(&mut headers, self.metadata)?;

        self.client
            .presign_async(
                Method::GET,
                &self.bucket,
                &self.key,
                self.expires_in,
                self.query_params,
                headers,
            )
            .await
    }
}

/// Request builder for presigned PUT requests.
pub struct PresignPutObjectRequest {
    client: Client,
    bucket: String,
    key: String,
    expires_in: Duration,
    query_params: Vec<(String, String)>,
    headers: HeaderMap,
    metadata: Vec<(String, String)>,
}

impl PresignPutObjectRequest {
    /// Sets the expiry duration.
    pub fn expires_in(mut self, duration: Duration) -> Self {
        self.expires_in = duration;
        self
    }

    /// Adds a query parameter to the presigned URL.
    pub fn query_param(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.query_params.push((name.into(), value.into()));
        self
    }

    /// Adds an HTTP header to sign.
    pub fn header(mut self, name: http::header::HeaderName, value: HeaderValue) -> Self {
        self.headers.insert(name, value);
        self
    }

    /// Adds a user metadata entry to sign.
    pub fn metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.push((key.into(), value.into()));
        self
    }

    /// Builds the presigned request using static credentials.
    pub fn build(self) -> Result<PresignedRequest> {
        let mut headers = self.headers;
        apply_metadata_headers(&mut headers, self.metadata)?;

        self.client.presign(
            Method::PUT,
            &self.bucket,
            &self.key,
            self.expires_in,
            self.query_params,
            headers,
        )
    }

    /// Builds the presigned request using the current credential provider.
    pub async fn build_async(self) -> Result<PresignedRequest> {
        let mut headers = self.headers;
        apply_metadata_headers(&mut headers, self.metadata)?;

        self.client
            .presign_async(
                Method::PUT,
                &self.bucket,
                &self.key,
                self.expires_in,
                self.query_params,
                headers,
            )
            .await
    }
}

/// Request builder for presigned HEAD requests.
pub struct PresignHeadObjectRequest {
    client: Client,
    bucket: String,
    key: String,
    expires_in: Duration,
    query_params: Vec<(String, String)>,
    headers: HeaderMap,
}

impl PresignHeadObjectRequest {
    /// Sets the expiry duration.
    pub fn expires_in(mut self, duration: Duration) -> Self {
        self.expires_in = duration;
        self
    }

    /// Adds a query parameter to the presigned URL.
    pub fn query_param(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.query_params.push((name.into(), value.into()));
        self
    }

    /// Adds an HTTP header to sign.
    pub fn header(mut self, name: http::header::HeaderName, value: HeaderValue) -> Self {
        self.headers.insert(name, value);
        self
    }

    /// Builds the presigned request using static credentials.
    pub fn build(self) -> Result<PresignedRequest> {
        self.client.presign(
            Method::HEAD,
            &self.bucket,
            &self.key,
            self.expires_in,
            self.query_params,
            self.headers,
        )
    }

    /// Builds the presigned request using the current credential provider.
    pub async fn build_async(self) -> Result<PresignedRequest> {
        self.client
            .presign_async(
                Method::HEAD,
                &self.bucket,
                &self.key,
                self.expires_in,
                self.query_params,
                self.headers,
            )
            .await
    }
}

/// Request builder for presigned DELETE requests.
pub struct PresignDeleteObjectRequest {
    client: Client,
    bucket: String,
    key: String,
    expires_in: Duration,
    query_params: Vec<(String, String)>,
    headers: HeaderMap,
}

impl PresignDeleteObjectRequest {
    /// Sets the expiry duration.
    pub fn expires_in(mut self, duration: Duration) -> Self {
        self.expires_in = duration;
        self
    }

    /// Adds a query parameter to the presigned URL.
    pub fn query_param(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.query_params.push((name.into(), value.into()));
        self
    }

    /// Adds an HTTP header to sign.
    pub fn header(mut self, name: http::header::HeaderName, value: HeaderValue) -> Self {
        self.headers.insert(name, value);
        self
    }

    /// Builds the presigned request using static credentials.
    pub fn build(self) -> Result<PresignedRequest> {
        self.client.presign(
            Method::DELETE,
            &self.bucket,
            &self.key,
            self.expires_in,
            self.query_params,
            self.headers,
        )
    }

    /// Builds the presigned request using the current credential provider.
    pub async fn build_async(self) -> Result<PresignedRequest> {
        self.client
            .presign_async(
                Method::DELETE,
                &self.bucket,
                &self.key,
                self.expires_in,
                self.query_params,
                self.headers,
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_xml_or_service_error_maps_error_xml_as_api_error() {
        let mut headers = HeaderMap::new();
        headers.insert("x-amz-request-id", HeaderValue::from_static("req-1"));
        let body = r#"
<Error>
  <Code>InternalError</Code>
  <Message>backend failure</Message>
</Error>
"#;

        let err = parse_xml_or_service_error::<()>(StatusCode::OK, &headers, body, |_xml| {
            Err(Error::decode("failed to parse success XML", None))
        })
        .expect_err("expected service error mapping");

        match err {
            Error::Api {
                status,
                code,
                message,
                request_id,
                ..
            } => {
                assert_eq!(status, StatusCode::OK);
                assert_eq!(code.as_deref(), Some("InternalError"));
                assert_eq!(message.as_deref(), Some("backend failure"));
                assert_eq!(request_id.as_deref(), Some("req-1"));
            }
            other => panic!("expected Api error, got {other:?}"),
        }
    }

    #[test]
    fn parse_xml_or_service_error_preserves_decode_error_for_plain_body() {
        let err = parse_xml_or_service_error::<()>(
            StatusCode::OK,
            &HeaderMap::new(),
            "not-xml",
            |_xml| Err(Error::decode("failed to parse success XML", None)),
        )
        .expect_err("expected parse failure");

        match err {
            Error::Decode { .. } => {}
            other => panic!("expected Decode error, got {other:?}"),
        }
    }

    #[test]
    fn put_stream_accepts_send_non_sync_streams() {
        use std::cell::Cell;
        use std::pin::Pin;
        use std::task::{Context, Poll};

        struct NonSyncStream {
            emitted: Cell<bool>,
        }

        #[derive(Debug)]
        struct UploadStreamError;

        impl std::fmt::Display for UploadStreamError {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("upload stream error")
            }
        }

        impl std::error::Error for UploadStreamError {}

        impl Stream for NonSyncStream {
            type Item = std::result::Result<Bytes, UploadStreamError>;

            fn poll_next(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
                let this = self.get_mut();
                if this.emitted.replace(true) {
                    Poll::Ready(None)
                } else {
                    Poll::Ready(Some(Ok(Bytes::from_static(b"x"))))
                }
            }
        }

        let client = Client::builder("https://s3.example.com")
            .expect("builder should parse")
            .region("us-east-1")
            .auth(crate::Auth::Anonymous)
            .build()
            .expect("client should build");

        let _request = client.objects().put("bucket", "key").body_stream_sized(
            NonSyncStream {
                emitted: Cell::new(false),
            },
            1,
        );
    }

    #[tokio::test]
    async fn put_bytes_rejects_mismatched_content_length() {
        let client = Client::builder("https://s3.example.com")
            .expect("builder should parse")
            .region("us-east-1")
            .auth(crate::Auth::Anonymous)
            .build()
            .expect("client should build");

        let err = client
            .objects()
            .put("bucket", "key")
            .content_length(4)
            .body_bytes("abc")
            .send()
            .await
            .expect_err("mismatched byte body content length must fail before transport");

        match err {
            Error::InvalidConfig { message } => assert!(message.contains("content_length")),
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
    }

    #[cfg(feature = "multipart")]
    #[test]
    fn validate_upload_part_body_rejects_empty() {
        let err = validate_upload_part_body(&AsyncBody::Empty).expect_err("expected invalid body");
        match err {
            Error::InvalidConfig { message } => {
                assert!(message.contains("upload_part requires a request body"));
            }
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
    }

    #[cfg(feature = "multipart")]
    #[test]
    fn validate_upload_part_number_rejects_out_of_range() {
        let err = validate_upload_part_number(0).expect_err("expected invalid part_number");
        match err {
            Error::InvalidConfig { message } => {
                assert!(message.contains("part_number"));
            }
            other => panic!("expected InvalidConfig, got {other:?}"),
        }

        let err = validate_upload_part_number(10_001).expect_err("expected invalid part_number");
        match err {
            Error::InvalidConfig { message } => {
                assert!(message.contains("part_number"));
            }
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
    }

    #[cfg(feature = "multipart")]
    #[test]
    fn validate_upload_id_rejects_empty() {
        let err = validate_upload_id("   ").expect_err("expected invalid upload_id");
        match err {
            Error::InvalidConfig { message } => {
                assert!(message.contains("upload_id"));
            }
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
    }

    #[cfg(feature = "multipart")]
    #[test]
    fn validate_max_parts_rejects_out_of_range() {
        let err = validate_max_parts(0).expect_err("expected invalid max_parts");
        match err {
            Error::InvalidConfig { message } => {
                assert!(message.contains("max_parts"));
            }
            other => panic!("expected InvalidConfig, got {other:?}"),
        }

        let err = validate_max_parts(1_001).expect_err("expected invalid max_parts");
        match err {
            Error::InvalidConfig { message } => {
                assert!(message.contains("max_parts"));
            }
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
    }

    #[test]
    fn validate_max_keys_rejects_out_of_range() {
        let err = validate_max_keys(0).expect_err("expected invalid max_keys");
        match err {
            Error::InvalidConfig { message } => {
                assert!(message.contains("max_keys"));
            }
            other => panic!("expected InvalidConfig, got {other:?}"),
        }

        let err = validate_max_keys(1_001).expect_err("expected invalid max_keys");
        match err {
            Error::InvalidConfig { message } => {
                assert!(message.contains("max_keys"));
            }
            other => panic!("expected InvalidConfig, got {other:?}"),
        }
    }
}
