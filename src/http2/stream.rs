use bytes::{Bytes, BytesMut};

use crate::http::{HTTPRequest, HeaderVal, HeadersMap};

use super::frames::{ContinuationFlags, DataFlags, ErrorCode, Frame, FrameBody, HeadersFlags};

use hpack::Decoder;
use hpack::Encoder;

/// Maintains a state machine and a vector of received frames
pub struct Stream {
    pub id: u32,
    pub state: StreamState,
    received_frames: Vec<Frame>,
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
/// See RFC 7540 section 8.1
enum ReqAssemblerState {
    Init,
    ReadingHeader,
    ReadingBody,
    ReadingTrailer,
    Done,
}

impl Stream {
    pub fn new(id: u32) -> Stream {
        Stream {
            id,
            state: StreamState::Idle,
            received_frames: vec![],
        }
    }

    fn assemble_request(&self) -> Result<HTTPRequest, ErrorCode> {
        let mut hdr_block = BytesMut::new();
        let mut body = BytesMut::new();
        let mut trailer_block = BytesMut::new();

        let mut state = ReqAssemblerState::Init;
        for frame in &self.received_frames {
            state = match state {
                ReqAssemblerState::Init => match &frame.payload {
                    FrameBody::Headers { hdr_block_frag, .. } => {
                        hdr_block.extend_from_slice(&hdr_block_frag);

                        let frame_flags: HeadersFlags =
                            HeadersFlags::from_bits_retain(frame.header.flags);

                        if frame_flags.contains(HeadersFlags::END_STREAM) {
                            ReqAssemblerState::Done
                        } else if frame_flags.contains(HeadersFlags::END_HEADERS) {
                            ReqAssemblerState::ReadingBody
                        } else {
                            ReqAssemblerState::ReadingHeader
                        }
                    }
                    _ => state,
                },

                ReqAssemblerState::ReadingHeader => match &frame.payload {
                    FrameBody::Continuation { hdr_block_frag } => {
                        hdr_block.extend_from_slice(&hdr_block_frag);

                        let frame_flags: ContinuationFlags =
                            ContinuationFlags::from_bits_retain(frame.header.flags);

                        if frame_flags.contains(ContinuationFlags::END_HEADERS) {
                            ReqAssemblerState::ReadingBody
                        } else {
                            state
                        }
                    }
                    _ => state,
                },

                ReqAssemblerState::ReadingBody => match &frame.payload {
                    FrameBody::Data { data, .. } => {
                        body.extend_from_slice(&data);

                        let frame_flags: DataFlags =
                            DataFlags::from_bits_retain(frame.header.flags);

                        if frame_flags.contains(DataFlags::END_STREAM) {
                            ReqAssemblerState::Done
                        } else {
                            state
                        }
                    }
                    FrameBody::Headers { hdr_block_frag, .. } => {
                        trailer_block.extend_from_slice(&hdr_block_frag);

                        let frame_flags: HeadersFlags =
                            HeadersFlags::from_bits_retain(frame.header.flags);

                        if frame_flags.contains(HeadersFlags::END_STREAM) {
                            ReqAssemblerState::Done
                        } else {
                            ReqAssemblerState::ReadingTrailer
                        }
                    }
                    _ => state,
                },

                ReqAssemblerState::ReadingTrailer => match &frame.payload {
                    FrameBody::Continuation { hdr_block_frag } => {
                        trailer_block.extend_from_slice(&hdr_block_frag);

                        let frame_flags: ContinuationFlags =
                            ContinuationFlags::from_bits_retain(frame.header.flags);

                        if frame_flags.contains(ContinuationFlags::END_HEADERS) {
                            ReqAssemblerState::Done
                        } else {
                            state
                        }
                    }
                    _ => state,
                },

                ReqAssemblerState::Done => break,
            }
        }
        // Assemble a Request struct from bytes read
        match state {
            ReqAssemblerState::Done => {
                let headers =
                    decompress_header(hdr_block.into()).map_err(|_| ErrorCode::CompressionError)?;
                let body: Option<Bytes> = match body.len() {
                    0 => None,
                    _ => Some(body.into()),
                };
                let trailers: Option<HeadersMap> = match trailer_block.len() {
                    0 => None,
                    _ => Some(
                        decompress_header(trailer_block.into())
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

        self.received_frames.push(frame);
        self.state = next_state;

        // TRY TODO: Support handling request before body is fully received

        if end_of_stream {
            self.assemble_request().map(|req| Some(req))
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

    let headers: Vec<(&[u8], &[u8])> = hdrs
        .into_iter()
        .map(|(key, value)| {
            (key.as_bytes(), value.as_bytes().clone().as_ref())
        })
        .collect();
    Bytes::from(encoder.encode(headers))
}

/// Decompress headers using HPACK.
/// See https://httpwg.org/specs/rfc7541.html.
pub fn decompress_header(bytes: Bytes) -> Result<HeadersMap, ErrorCode> {
    let mut decoder: Decoder = Decoder::new();
    match decoder.decode(&bytes) {
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
            decompress_header(compressed);

        assert_eq!(decompressed.unwrap(), hdrs);
    }
}
