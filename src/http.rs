use std::collections::HashMap;
use std::mem;

use bytes::Bytes;

/// Calculate the size of an uncompressed headers list in octets(bytes).
/// See https://httpwg.org/specs/rfc7540.html#SETTINGS_MAX_HEADER_LIST_SIZE.
pub fn hdr_map_size(hdr_map: HeadersMap) -> usize {
    let mut size: usize = 0;
    for (key, value) in &hdr_map {
        // 32 octet overhead for each entry.
        size += 32;

        // Add size of header name.
        size += mem::size_of_val(key);

        // Add size of header value(s).
        match value {
            HeaderVal::Single(val) => size += mem::size_of_val(val),
            HeaderVal::Multiple(vals) => {
                for val in vals {
                    size += mem::size_of_val(val);
                }
            }
        }
    }

    size
}

#[derive(Debug, PartialEq)]
pub enum HeaderVal {
    Single(String),
    Multiple(Vec<String>),
}

pub type HeadersMap = HashMap<String, HeaderVal>;

pub trait ReqHandlerFn: Fn(HTTPRequest) -> HTTPResponse {}
impl<F: Fn(HTTPRequest) -> HTTPResponse> ReqHandlerFn for F {}

pub struct HTTPRequest {
    pub method: String,
    pub path: String,
    pub protocol: String,

    pub headers: HeadersMap,
    pub body: Option<Bytes>,
    pub trailers: Option<HeadersMap>,
}
impl HTTPRequest {
    pub fn new(method: &str, path: &str, protocol: &str) -> Self {
        HTTPRequest {
            method: String::from(method),
            path: String::from(path),
            protocol: String::from(protocol),
            headers: HeadersMap::new(),
            body: None,
            trailers: None,
        }
    }
}

#[derive(Debug)]
pub struct HTTPResponse {
    pub status: ResponseStatus,

    pub headers: HeadersMap,
    pub body: Option<Bytes>,
    pub trailers: Option<HeadersMap>,
}
impl HTTPResponse {
    pub fn default() -> Self {
        // Testing purposes only
        Self {
            status: ResponseStatus::Ok,
            headers: HeadersMap::new(),
            body: None,
            trailers: None,
        }
    }
}

#[derive(Debug)]
#[repr(u32)]
pub enum ResponseStatus {
    // Informational
    Continue = 100,
    SwitchingProtocols = 101,

    // Successful
    Ok = 200,
    Created = 201,
    Accepted = 202,
    NonAuthoritativeInformation = 203,
    NoContent = 204,
    ResetContent = 205,
    PartialContent = 206,

    // Redirection
    MultipleChoices = 300,
    MovedPermanently = 301,
    Found = 302,
    SeeOther = 303,
    NotModified = 304,
    UseProxy = 305,
    TemporaryRedirect = 307,
    PermanentRedirect = 308,

    // Client Error
    BadRequest = 400,
    Unauthorized = 401,
    PaymentRequired = 402,
    Forbidden = 403,
    NotFound = 404,
    MethodNotAllowed = 405,
    NotAcceptable = 406,
    ProxyAuthenticationRequired = 407,
    RequestTimeout = 408,
    Conflict = 409,
    Gone = 410,
    LengthRequired = 411,
    PreconditionFailed = 412,
    ContentTooLarge = 413,
    UriTooLong = 414,
    UnsupportedMediaType = 415,
    RangeNotSatisfiable = 416,
    ExpectationFailed = 417,
    MisdirectedRequest = 421,
    UnprocessableContent = 422,
    UpgradeRequired = 426,

    // Server Error
    InternalServerError = 500,
    NotImplemented = 501,
    BadGateway = 502,
    ServiceUnavaliable = 503,
    GatewayTimeout = 504,
    HTTPVersionNotSupported = 505,
}

mod tests {
    use super::*;

    #[test]
    fn find_uncompressed_header_list_size_correctly() {
        let mut hdr_map: HashMap<String, HeaderVal> = HeadersMap::new();

        let header_name_1 = String::from("Accept");
        let header_value_1 = HeaderVal::Single(String::from("text/html"));
        let header_name_2 = String::from("Accept-Encoding");
        let header_value_2 = HeaderVal::Multiple(vec![
            String::from("gzip"),
            String::from("deflate"),
            String::from("br"),
        ]);
        let header_name_3 = String::from("Accept-Language");
        let header_value_3 = HeaderVal::Single(String::from("en-US"));
        let header_name_4 = String::from("Cache-Control");
        let header_value_4 = HeaderVal::Single(String::from("max-age=0"));
        let header_name_5 = String::from("Connection");
        let header_value_5 = HeaderVal::Single(String::from("keep-alive"));
        let header_name_6 = String::from("Cookie");
        let haader_value_6 = HeaderVal::Single(String::from("cookie1=foo; cookie2=bar"));
        let header_name_7 = String::from("Host");
        let header_value_7 = HeaderVal::Single(String::from("localhost:8080"));
        let header_name_8 = String::from("Upgrade-Insecure-Requests");
        let header_value_8 = HeaderVal::Single(String::from("1"));
        let header_name_9 = String::from("User-Agent");
        let header_value_9 = HeaderVal::Single(String::from("Chrome/51.0.2704.103"));

        hdr_map.insert(header_name_1, header_value_1);
        hdr_map.insert(header_name_2, header_value_2);
        hdr_map.insert(header_name_3, header_value_3);
        hdr_map.insert(header_name_4, header_value_4);
        hdr_map.insert(header_name_5, header_value_5);
        hdr_map.insert(header_name_6, haader_value_6);
        hdr_map.insert(header_name_7, header_value_7);
        hdr_map.insert(header_name_8, header_value_8);
        hdr_map.insert(header_name_9, header_value_9);

        let actual_size = mem::size_of_val(&header_name_1)
            + mem::size_of_val(&header_value_1)
            + mem::size_of_val(&header_name_2)
            + mem::size_of_val(&header_value_2)
            + mem::size_of_val(&header_name_3)
            + mem::size_of_val(&header_value_3)
            + mem::size_of_val(&header_name_4)
            + mem::size_of_val(&header_value_4)
            + mem::size_of_val(&header_name_5)
            + mem::size_of_val(&header_value_5)
            + mem::size_of_val(&header_name_6)
            + mem::size_of_val(&haader_value_6)
            + mem::size_of_val(&header_name_7)
            + mem::size_of_val(&header_value_7)
            + mem::size_of_val(&header_name_8)
            + mem::size_of_val(&header_value_8)
            + mem::size_of_val(&header_name_9)
            + mem::size_of_val(&header_value_9)
            + 32 * 9;

        assert_eq!(hdr_map_size(hdr_map), actual_size);
    }
}
