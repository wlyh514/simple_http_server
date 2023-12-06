use bytes::{Bytes, BytesMut};

use crate::http::{HTTPRequest, HeaderVal, HeadersMap};

use super::frames::{ContinuationFlags, DataFlags, ErrorCode, Frame, FrameBody, HeadersFlags};

use hpack::Decoder;
use hpack::Encoder;

/// See RFC 7540 section 8.1
#[derive(PartialEq, Debug)]
pub enum ReqAssemblerState {
    Init,
    ReadingHeader,
    ReadingBody,
    ReadingTrailer,
    Done,
}

struct ReqAssembler {
    hdr_block: BytesMut,
    body: BytesMut, 
    trailer_block: BytesMut,
    state: ReqAssemblerState,
}
impl ReqAssembler {
    pub fn new() -> Self {
        Self {
            hdr_block: BytesMut::new(),
            body: BytesMut::new(), 
            trailer_block: BytesMut::new(),
            state: ReqAssemblerState::Init,
        }
    }

    pub fn recv_frame(&mut self, frame: Frame) {
        let new_state: Option<ReqAssemblerState> = match self.state {
            ReqAssemblerState::Init => match &frame.payload {
                FrameBody::Headers { hdr_block_frag, .. } => {
                    self.hdr_block.extend_from_slice(&hdr_block_frag);

                    let frame_flags: HeadersFlags =
                        HeadersFlags::from_bits_retain(frame.header.flags);

                    if frame_flags.contains(HeadersFlags::END_STREAM) {
                        Some(ReqAssemblerState::Done)
                    } else if frame_flags.contains(HeadersFlags::END_HEADERS) {
                        Some(ReqAssemblerState::ReadingBody)
                    } else {
                        Some(ReqAssemblerState::ReadingHeader)
                    }
                }
                _ => None,
            },

            ReqAssemblerState::ReadingHeader => match &frame.payload {
                FrameBody::Continuation { hdr_block_frag } => {
                    self.hdr_block.extend_from_slice(&hdr_block_frag);

                    let frame_flags: ContinuationFlags =
                        ContinuationFlags::from_bits_retain(frame.header.flags);

                    if frame_flags.contains(ContinuationFlags::END_HEADERS) {
                        Some(ReqAssemblerState::ReadingBody)
                    } else {
                        None
                    }
                }
                _ => None,
            },

            ReqAssemblerState::ReadingBody => match &frame.payload {
                FrameBody::Data { data, .. } => {
                    self.body.extend_from_slice(&data);

                    let frame_flags: DataFlags =
                        DataFlags::from_bits_retain(frame.header.flags);

                    if frame_flags.contains(DataFlags::END_STREAM) {
                        Some(ReqAssemblerState::Done)
                    } else {
                        None
                    }
                }
                FrameBody::Headers { hdr_block_frag, .. } => {
                    self.trailer_block.extend_from_slice(&hdr_block_frag);

                    let frame_flags: HeadersFlags =
                        HeadersFlags::from_bits_retain(frame.header.flags);

                    if frame_flags.contains(HeadersFlags::END_STREAM) {
                        Some(ReqAssemblerState::Done)
                    } else {
                        Some(ReqAssemblerState::ReadingTrailer)
                    }
                }
                _ => None,
            },

            ReqAssemblerState::ReadingTrailer => match &frame.payload {
                FrameBody::Continuation { hdr_block_frag } => {
                    self.trailer_block.extend_from_slice(&hdr_block_frag);
                    let frame_flags: ContinuationFlags =
                        ContinuationFlags::from_bits_retain(frame.header.flags);

                    if frame_flags.contains(ContinuationFlags::END_HEADERS) {
                        Some(ReqAssemblerState::Done)
                    } else {
                        None 
                    }
                }
                _ => None,
            },

            ReqAssemblerState::Done => None,
        };
        if let Some(new_state) = new_state {
            self.state = new_state
        };
    }

    pub fn assemble(&mut self) -> Result<HTTPRequest, ErrorCode>{
        match self.state {
            ReqAssemblerState::Done => {
                let headers =
                    decompress_header(&self.hdr_block).map_err(|_| ErrorCode::CompressionError)?;

                let body: Option<Bytes> = match self.body.len() {
                    0 => None,
                    _ => Some(self.body.clone().into()),
                };
                let trailers: Option<HeadersMap> = match self.trailer_block.len() {
                    0 => None,
                    _ => Some(
                        decompress_header(&self.trailer_block)
                            .map_err(|_| ErrorCode::CompressionError)?,
                    ),
                };

                // Validate request, see RFC7540 section 8.1.2.6
                let method = hdr_field_try_get_single_val(&headers, ":method")?;
                let _scheme = hdr_field_try_get_single_val(&headers, ":scheme")?;
                let path = hdr_field_try_get_single_val(&headers, ":path")?;

                let mut req = HTTPRequest::new(method.as_str(), path.as_str(), "HTTP/2");
                req.headers = headers;
                req.body = body;
                req.trailers = trailers;
                Ok(req)
            }
            _ => Err(ErrorCode::ProtocolError),
        }
    }

    pub fn get_state(&self) -> &ReqAssemblerState {
        &self.state
    }
}

/// Maintains a state machine and a vector of received frames
pub struct Stream {
    pub id: u32,
    pub state: StreamState,
    req_assembler: ReqAssembler,
}
#[derive(PartialEq, Clone, Copy)]
pub enum StreamState {
    Idle,
    Open,
    ReservedLocal,
    ReservedRemote,
    HalfClosedRemote,
    HalfClosedLocal,
    Closed,
}


impl Stream {
    pub fn new(id: u32) -> Stream {
        Stream {
            id,
            state: StreamState::Idle,
            req_assembler: ReqAssembler::new()
        }
    }

    pub fn get_req_assembler_state(&self) -> &ReqAssemblerState {
        self.req_assembler.get_state()
    }

    pub fn recv(&mut self, frame: Frame) -> Result<Option<HTTPRequest>, ErrorCode> {
        let end_of_stream: bool = match frame.payload {
            FrameBody::Headers { .. } => {
                let hdr_flags: HeadersFlags = HeadersFlags::from_bits_retain(frame.header.flags);
                hdr_flags.contains(HeadersFlags::END_STREAM)
            }
            FrameBody::Data { .. } => {
                let hdr_flags: DataFlags = DataFlags::from_bits_retain(frame.header.flags);
                hdr_flags.contains(DataFlags::END_STREAM)
            }
            _ => false,
        };

        // The state machine defined in section 5.1
        let next_state: StreamState = match self.state {
            StreamState::Idle => match frame.payload {
                FrameBody::PushPromise { .. } => StreamState::ReservedRemote,
                FrameBody::Headers { .. } => StreamState::Open,
                _ => self.state,
            },

            StreamState::Open => {
                if end_of_stream {
                    StreamState::HalfClosedRemote
                } else {
                    match frame.payload {
                        FrameBody::RstStream { .. } => StreamState::Closed,
                        _ => self.state,
                    }
                }
            }

            StreamState::ReservedRemote => match frame.payload {
                FrameBody::RstStream { .. } => StreamState::Closed,
                _ => self.state,
            },

            StreamState::ReservedLocal => match frame.payload {
                FrameBody::RstStream { .. } => StreamState::Closed,
                _ => self.state,
            },

            StreamState::HalfClosedRemote => match frame.payload {
                FrameBody::RstStream { .. } => StreamState::Closed,
                _ => self.state,
            },

            StreamState::HalfClosedLocal => {
                if end_of_stream {
                    StreamState::Closed
                } else {
                    match frame.payload {
                        FrameBody::RstStream { .. } => StreamState::Closed,
                        _ => self.state,
                    }
                }
            }

            StreamState::Closed => self.state,
        };
        self.state = next_state;

        // TRY TODO: Support handling request before body is fully received

        self.req_assembler.recv_frame(frame);
        if end_of_stream {
            self.req_assembler.assemble().map(|req| Some(req))
        } else {
            Ok(None)
        }
    }

    pub fn send(&mut self, frame: &Frame) {
        let end_of_stream: bool = match frame.payload {
            FrameBody::Headers { .. } => {
                let hdr_flags: HeadersFlags = HeadersFlags::from_bits_retain(frame.header.flags);
                hdr_flags.contains(HeadersFlags::END_STREAM)
            }
            FrameBody::Data { .. } => {
                let hdr_flags: DataFlags = DataFlags::from_bits_retain(frame.header.flags);
                hdr_flags.contains(DataFlags::END_STREAM)
            }
            _ => false,
        };

        // The state machine defined in section 5.1
        let new_state: StreamState = match self.state {
            StreamState::Idle => {
                match frame.payload {
                    FrameBody::PushPromise { .. } => StreamState::ReservedLocal, 
                    FrameBody::Headers { .. } => StreamState::Open, 
                    _ => self.state,
                }
            }, 

            StreamState::Open => {
                if end_of_stream {
                    StreamState::HalfClosedLocal
                } else {
                    match frame.payload {
                        FrameBody::RstStream { .. } => StreamState::Closed,
                        _ => self.state, 
                    }
                }
            },

            StreamState::ReservedLocal => {
                match frame.payload {
                    FrameBody::Headers { .. } => StreamState::HalfClosedRemote,
                    FrameBody::RstStream { .. } => StreamState::Closed,
                    _ => self.state,
                }
            },

            StreamState::HalfClosedRemote => {
                if end_of_stream {
                    StreamState::Closed
                } else {
                    match frame.payload {
                        FrameBody::RstStream { .. } => StreamState::Closed,
                        _ => self.state,
                    }
                }
            },

            StreamState::ReservedRemote => {
                match frame.payload {
                    FrameBody::RstStream { .. } => StreamState::Closed,
                    _ => self.state,
                }
            },

            StreamState::HalfClosedLocal => {
                match frame.payload {
                    FrameBody::RstStream { .. } => StreamState::Closed,
                    _ => self.state, 
                }
            },

            StreamState::Closed => self.state
        };

        self.state = new_state;
    }
}

/// Checks if field_name of hdr_map contains exactly one value. If true, return Some(value)
fn hdr_field_try_get_single_val(hdr_map: &HeadersMap, field_name: &str) -> Result<String, ErrorCode> {
    let vals = hdr_map.get(field_name);
    match vals {
        Some(vals) => match vals {
            HeaderVal::Single(val) => Ok(val.to_string()),
            _ => Err(ErrorCode::ProtocolError),
        },
        None => Err(ErrorCode::ProtocolError),
    }
}
/// Compress headers using HPACK.
/// See https://httpwg.org/specs/rfc7541.html.
pub fn compress_header(hdrs: &HeadersMap) -> Bytes {
    let mut encoder: Encoder<'_> = Encoder::new();
    let mut header_vec: Vec<(Vec<u8>, Vec<u8>)> = hdrs.into_iter().map(|(k, v)| (k.as_bytes().to_vec(), v.as_bytes().to_vec())).collect();
    header_vec.sort();
    return Bytes::from(encoder.encode(header_vec.iter().map(|h| (&h.0[..], &h.1[..]))));
    // Bytes::from(encoder.encode(hdrs.iter().map(|(k, v)| (k.as_bytes(), v.as_bytes()))))
}

/// Decompress headers using HPACK.
/// See https://httpwg.org/specs/rfc7541.html.
pub fn decompress_header(bytes: &[u8]) -> Result<HeadersMap, ErrorCode> {
    let mut decoder: Decoder = Decoder::new();
    match decoder.decode(bytes) {
        Ok(header_list) => {
            let mut hdrs: HeadersMap = HeadersMap::new();
            for (key, value) in header_list {
                let key_str: String =
                    String::from_utf8(key.to_vec()).map_err(|_| ErrorCode::CompressionError)?;
                let value_str: String =
                    String::from_utf8(value.to_vec()).map_err(|_| ErrorCode::CompressionError)?;
                hdrs.insert(key_str, HeaderVal::Single(value_str));
            }
            Ok(hdrs)
        }
        Err(_) => Err(ErrorCode::CompressionError),
    }
}

mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn compress_header_correctly() {
        let mut hdrs: HashMap<String, HeaderVal> = HeadersMap::new();
        hdrs.insert(
            String::from(":method"),
            HeaderVal::Single(String::from("GET")),
        );
        hdrs.insert(String::from(":path"), HeaderVal::Single(String::from("/")));

        let compressed: Bytes = compress_header(&hdrs);

        // The headers are encoded by providing their index (with a bit flag
        // indicating that the indexed representation is used).
        assert_eq!(compressed, vec![2 | 0x80, 4 | 0x80]);
    }

    #[test]
    fn decompress_header_correctly() {
        let mut hdrs: HashMap<String, HeaderVal> = HeadersMap::new();
        hdrs.insert(
            String::from(":method"),
            HeaderVal::Single(String::from("GET")),
        );
        hdrs.insert(String::from(":path"), HeaderVal::Single(String::from("/")));

        let compressed: Bytes = compress_header(&hdrs);
        let decompressed: Result<HashMap<String, HeaderVal>, ErrorCode> =
            decompress_header(&compressed);

        assert_eq!(decompressed.unwrap(), hdrs);
    }
}
