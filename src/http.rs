use std::collections::HashMap;

use bytes::Bytes;

pub type HeadersMap = HashMap<String, Vec<String>>;
pub trait ReqHandlerFn: Fn(HTTPRequest) -> HTTPResponse {}

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
    status: ResponseStatus,
    protocol: String,

    headers: HeadersMap,
    body: Option<Bytes>,
    trailers: Option<HeadersMap>,
}

#[repr(u32)]
enum ResponseStatus {
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

    Custom(u32),
}