use ::std::net::TcpStream;

use bytes::{BytesMut, Bytes};

use crate::http::{HTTPRequest, HeadersMap};

use super::frames::{Frame, FrameBody, HeadersFlags, ContinuationFlags, DataFlags, ErrorCode};

pub struct Stream {
    id: i32,
    state: StreamState,
    tcp_stream: TcpStream,
    received_frames: Vec<Frame>,
}
enum StreamState {
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
    fn new(id: i32, tcp_stream: TcpStream) -> Stream {
        Stream {
            id,
            state: StreamState::Idle,
            tcp_stream,
            received_frames: vec![]
        }
    }

    fn assemble_request(self) -> Result<HTTPRequest, ErrorCode> {
        let mut hdr_block = BytesMut::new();
        let mut body = BytesMut::new();
        let mut trailer_block = BytesMut::new();

        let mut state = ReqAssemblerState::Init;
        for frame in self.received_frames {
            state = match state {
                ReqAssemblerState::Init => {
                    match frame.payload {
                        FrameBody::Headers { hdr_block_frag, .. } => {
                            hdr_block.extend_from_slice(&hdr_block_frag);
                            
                            let frame_flags: HeadersFlags = HeadersFlags::from_bits_retain(frame.header.flags);

                            if frame_flags.contains(HeadersFlags::END_STREAM) {
                                ReqAssemblerState::Done
                            } else if frame_flags.contains(HeadersFlags::END_HEADERS) {
                                ReqAssemblerState::ReadingBody
                            } else {
                                ReqAssemblerState::ReadingHeader
                            }
                        },
                        _ => state
                    }
                },

                ReqAssemblerState::ReadingHeader => {
                    match frame.payload {
                        FrameBody::Continuation { hdr_block_frag } => {
                            hdr_block.extend_from_slice(&hdr_block_frag);

                            let frame_flags: ContinuationFlags = ContinuationFlags::from_bits_retain(frame.header.flags); 

                            if frame_flags.contains(ContinuationFlags::END_HEADERS) {
                                ReqAssemblerState::ReadingBody
                            } else {
                                state
                            }
                        },
                        _ => state
                    }
                },

                ReqAssemblerState::ReadingBody => {
                    match frame.payload {
                        FrameBody::Data { data, .. } => {
                            body.extend_from_slice(&data); 

                            let frame_flags: DataFlags = DataFlags::from_bits_retain(frame.header.flags);

                            if frame_flags.contains(DataFlags::END_STREAM) {
                                ReqAssemblerState::Done
                            } else {
                                state
                            }
                        },
                        FrameBody::Headers { hdr_block_frag, .. } => {
                            trailer_block.extend_from_slice(&hdr_block_frag);
                            
                            let frame_flags: HeadersFlags = HeadersFlags::from_bits_retain(frame.header.flags); 

                            if frame_flags.contains(HeadersFlags::END_STREAM) {
                                ReqAssemblerState::Done
                            } else {
                                ReqAssemblerState::ReadingTrailer
                            }
                        },
                        _ => state
                    }
                },

                ReqAssemblerState::ReadingTrailer => {
                    match frame.payload {
                        FrameBody::Continuation { hdr_block_frag } => {
                            trailer_block.extend_from_slice(&hdr_block_frag);

                            let frame_flags: ContinuationFlags = ContinuationFlags::from_bits_retain(frame.header.flags);

                            if frame_flags.contains(ContinuationFlags::END_HEADERS) {
                                ReqAssemblerState::Done
                            } else {
                                state
                            }
                        },
                        _ => state
                    }
                },

                ReqAssemblerState::Done => break,
            }
        }
        // Assemble a Request struct from bytes read
        match state {
            ReqAssemblerState::Done => {
                let headers = decompress_header(hdr_block.into()).map_err(|_| ErrorCode::CompressionError)?;
                let body: Option<Bytes> = match body.len() {
                    0 => None, 
                    _ => Some(body.into()),
                };
                let trailer: Option<HeadersMap> = match trailer_block.len() {
                    0 => None, 
                    _ => Some(decompress_header(trailer_block.into()).map_err(|_| ErrorCode::CompressionError)?),
                };

                // Validate request, see RFC7540 section 8.1.2.6
                let method = hdr_field_try_get_single_val(headers, ":method")?;
                let _scheme = hdr_field_try_get_single_val(headers, ":scheme")?;
                let path = hdr_field_try_get_single_val(headers, ":path")?;

                if body.is_some() && !body.unwrap().is_empty() {
                    
                }

                Ok(HTTPRequest::new(method, path, "HTTP/2"))
            },
            _ => Err(ErrorCode::ProtocolError)
        }
    }

    fn recv(mut self, frame: Frame) {
        // TODO: Implement this
        
    }

    fn send(self, frame: Frame) {
        // TODO: Implement this
    }
}


/// Checks if field_name of hdr_map contains exactly one value. If true, return Some(value)
fn hdr_field_try_get_single_val(hdr_map: HeadersMap, field_name: &str) -> Result<&str, ErrorCode> {
    let vals = hdr_map.get(field_name);
    match vals {
        Some(vals) => {
            match vals.len() {
                1 => Ok(vals.get(0).unwrap()), 
                _ => Err(ErrorCode::ProtocolError),
            }
        }, 
        None => Err(ErrorCode::ProtocolError),
    }
}

fn compress_header(hdrs: HeadersMap) -> Result<Bytes, ()> {
    // TODO: Implement this
    Err(())
}

fn decompress_header(bytes: Bytes) -> Result<HeadersMap, ()> {
    // TODO: Implement this
    Err(())
}

// https://prod.liveshare.vsengsaas.visualstudio.com/join?E3D9760360C65129C29CB3071F23E6C3534B