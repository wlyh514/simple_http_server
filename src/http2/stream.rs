use ::std::net::TcpStream;

use bytes::{BytesMut, Bytes};

use crate::http::{HTTPRequest, HeadersMap};

use super::frames::{Frame, FrameBody, HeadersFlags};

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

    fn assemble_request(self) -> Result<HTTPRequest, ()> {
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

                            
                            state
                        },
                        FrameBody::Data { data, .. } => {
                            state
                        }
                    }
                }
                _ => state,
            }
        }
        match state {
            ReqAssemblerState::Done => {
                let headers = decompress_header(hdr_block.into())?;
                let body: Option<Bytes> = match body.len() {
                    0 => None, 
                    _ => Some(body.into()),
                };
                let trailer: Option<HeadersMap> = match trailer_block.len() {
                    0 => None, 
                    _ => Some(decompress_header(trailer_block.into())?)
                };
                let method = headers.get(":method").map_or(Err(()), |val| Ok(val))?.as_str();
                // Ignore scheme and authority for now
                let path = headers.get(":path").map_or(Err(()), |val| Ok(val))?.as_str();
                Ok(HTTPRequest::new(method, path, "HTTP/2"))
            },
            _ => Err(())
        }
    }

    fn recv(mut self, frame: Frame) {
        // TODO: Implement this
    }

    fn send(self, frame: Frame) {
        // TODO: Implement this
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