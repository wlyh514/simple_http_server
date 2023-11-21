use std::collections::HashMap;

use bytes::Bytes;

pub type HeadersMap = HashMap<String, String>;

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
            trailers: None
        }
    }
}

pub struct HTTPResponse {
    status: ResponseStatusCode,
    protocol: String,

    headers: HeadersMap,
    body: Option<Bytes>,
    trailers: Option<HeadersMap>,
}

enum ResponseStatusCode {
    // TODO
}
