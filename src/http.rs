use std::mem;
use std::{collections::HashMap, fs};

use bytes::{Bytes, BytesMut};

/// Calculate the size of an uncompressed headers list in octets(bytes).
/// See https://httpwg.org/specs/rfc7540.html#SETTINGS_MAX_HEADER_LIST_SIZE.
pub fn hdr_map_size(hdr_map: &HeadersMap) -> usize {
    let mut size: usize = 0;
    for (key, value) in hdr_map {
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

#[derive(Debug, PartialEq, Clone)]
pub enum HeaderVal {
    Single(String),
    Multiple(Vec<String>),
}
impl HeaderVal {
    pub fn as_bytes(&self) -> Bytes {
        let mut bytes = BytesMut::new();
        match self {
            Self::Single(val) => bytes.extend_from_slice(val.as_bytes()),
            Self::Multiple(vals) => bytes.extend_from_slice(vals.join(",").as_bytes()),
        }
        bytes.into()
    }
}

pub type HeadersMap = HashMap<String, HeaderVal>;

pub trait ReqHandlerFn: Fn(HTTPRequest, &mut HTTPResponse) -> () + Sync + Send {}
impl<F: Fn(HTTPRequest, &mut HTTPResponse) -> () + Sync + Send> ReqHandlerFn for F {}

#[derive(Debug)]
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
        let mut resp = Self {
            status: ResponseStatus::Ok,
            headers: HeadersMap::new(),
            body: None,
            trailers: None,
        };
        resp.set("Content-Length", "0");
        resp
    }
    pub fn set(&mut self, field: &str, value: &str) {
        self.headers
            .insert(field.into(), HeaderVal::Single(value.into()));
    }

    pub fn set_multiple(&mut self, values: HashMap<&str, &str>) {
        for (field, value) in values {
            self.headers
                .insert(field.into(), HeaderVal::Single(value.into()));
        }
    }

    pub fn file(&mut self, fp: &str) {
        match fs::read(fp) {
            Ok(content) => {
                self.bytes(Bytes::from(content));
                self.status(ResponseStatus::Ok);
            }
            Err(_) => {
                self.file("static/404.html");
                self.status(ResponseStatus::NotFound);
            }
        };
    }

    pub fn status(&mut self, status: ResponseStatus) {
        self.status = status;
    }

    pub fn text(&mut self, text: String) {
        self.bytes(Bytes::from(text));
    }

    pub fn bytes(&mut self, body: Bytes) {
        let content_len = body.len();
        self.body = Some(body);
        self.set("Content-Length", &format!("{content_len}"));
    }
}

#[derive(Debug, Clone)]
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

#[cfg(test)]
mod tests {
    use super::*;

    fn actual_sizeof_val(val: &HeaderVal) -> usize {
        match val {
            HeaderVal::Single(val) => mem::size_of_val(val),
            HeaderVal::Multiple(vals) => {
                let mut size = 0;
                for val in vals {
                    size += mem::size_of_val(val);
                }
                size
            }
        }
    }
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

        hdr_map.insert(header_name_1.clone(), header_value_1.clone());
        hdr_map.insert(header_name_2.clone(), header_value_2.clone());
        hdr_map.insert(header_name_3.clone(), header_value_3.clone());
        hdr_map.insert(header_name_4.clone(), header_value_4.clone());
        hdr_map.insert(header_name_5.clone(), header_value_5.clone());
        hdr_map.insert(header_name_6.clone(), haader_value_6.clone());
        hdr_map.insert(header_name_7.clone(), header_value_7.clone());
        hdr_map.insert(header_name_8.clone(), header_value_8.clone());
        hdr_map.insert(header_name_9.clone(), header_value_9.clone());

        let actual_size = mem::size_of_val(&header_name_1)
            + actual_sizeof_val(&header_value_1)
            + mem::size_of_val(&header_name_2)
            + actual_sizeof_val(&header_value_2)
            + mem::size_of_val(&header_name_3)
            + actual_sizeof_val(&header_value_3)
            + mem::size_of_val(&header_name_4)
            + actual_sizeof_val(&header_value_4)
            + mem::size_of_val(&header_name_5)
            + actual_sizeof_val(&header_value_5)
            + mem::size_of_val(&header_name_6)
            + actual_sizeof_val(&haader_value_6)
            + mem::size_of_val(&header_name_7)
            + actual_sizeof_val(&header_value_7)
            + mem::size_of_val(&header_name_8)
            + actual_sizeof_val(&header_value_8)
            + mem::size_of_val(&header_name_9)
            + actual_sizeof_val(&header_value_9)
            + 32 * 9;

        assert_eq!(hdr_map_size(&hdr_map), actual_size);
    }
}
