use ::std::{io::BufReader, net::TcpStream};

use ::bitflags::bitflags;
use ::bytes::{BufMut, Bytes, BytesMut};

use super::connection::Settings;
use crate::http::HeadersMap;

fn compress_headers(headers: HeadersMap) -> Bytes {
    // TODO: Implement this
}

fn decompress_headers(header_block: Bytes) -> HeadersMap {
    // TODO: Implement this
}

bitflags! {
    struct DataFlags: u8 {
        const END_STREAM = 0x1;
        const PADDED = 0x8;
    }

    struct HeadersFlags: u8 {
        const END_STREAM = 0x1;
        const END_HEADERS = 0x4;
        const PADDED = 0x8;
        const PRIORITY = 0x20;
    }

    struct SettingsFlags: u8 {
        const ACK = 0x1;
    }

    struct PushPromiseFlags: u8 {
        const END_HEADERS = 0x4;
        const PADDED = 0x8;
    }

    struct PingFlags: u8 {
        const ACK = 0x1;
    }

    struct ContinuationFlags: u8 {
        const END_HEADERS = 0x4;
    }
}

struct H2HeadersMap(HeadersMap);
impl Into<Bytes> for H2HeadersMap {
    fn into(self) -> Bytes {
        compress_headers(self.0)
    }
}
impl H2HeadersMap {
    fn merge(mut self, other: H2HeadersMap) {
        for key in other.0.keys() {
            self.0.insert(*key, *other.0.get(key).unwrap());
        }
    }
}

const FRAME_HDR_SIZE: usize = 4 + 8 + 8 + 32;
/// See RFC7540 section 4
struct FrameHeader {
    length: usize,
    frame_type: u8,
    flags: u8,
    stream_id: u32,
}
impl FrameHeader {
    fn put_buf(self, mut buf: BytesMut) {
        // Write length as u24 in big endian
        buf.put_u8(((self.length >> 16) & 0xff) as u8);
        buf.put_u8(((self.length >> 8) & 0xff) as u8);
        buf.put_u8((self.length & 0xff) as u8);

        buf.put_u8(self.frame_type);
        buf.put_u8(self.flags);
        buf.put_u32(self.stream_id & 0x7fff_ffff);
    }
}

/// See RFC7540 section 6
#[repr(u8)]
enum FrameBody {
    Data {
        pad_length: usize,
        data: Bytes,
    },
    Headers {
        pad_length: usize,
        e: bool,
        stream_dep: u32,
        weight: u8,
        hdr_block_frag: H2HeadersMap,
    },
    Priority {
        e: bool,
        stream_pri: u32,
        weight: u8,
    },
    RstStream {
        error_code: ErrorCodes,
    },
    Settings {
        identifier: Settings,
        value: u32,
    },
    PushPromise {
        pad_length: usize,
        promised_stream_id: u32,
        hdr_block_frag: H2HeadersMap,
    },
    Ping {
        data: Bytes,
    },
    GoAway {
        last_stream_id: u32,
        error_code: ErrorCodes,
        additional_debug_data: Bytes,
    },
    WindowUpdate {
        window_size_increment: u32,
    },
    Continuation {
        hdr_block_frag: H2HeadersMap,
    },
}
impl FrameBody {
    fn to_id(self) -> u8 {
        match self {
            Self::Data { .. } => 0x0,
            Self::Headers { .. } => 0x1,
            Self::Priority { .. } => 0x2,
            Self::RstStream { .. } => 0x3,
            Self::Settings { .. } => 0x4,
            Self::PushPromise { .. } => 0x5,
            Self::Ping { .. } => 0x6,
            Self::GoAway { .. } => 0x7,
            Self::WindowUpdate { .. } => 0x8,
            Self::Continuation { .. } => 0x9,
        }
    }

    fn size(self) -> usize {
        // TODO: Implement this
        0
    }

    fn put_buf(self, mut buf: BytesMut) {
        match self {
            Self::Data { pad_length, data } => {
                buf.put_u8(pad_length.try_into().unwrap());
                buf.put(data);
                buf.put_bytes(0, pad_length);
            }
            Self::Headers {
                pad_length,
                e,
                stream_dep,
                weight,
                hdr_block_frag,
            } => {
                let hdr_block_frag_bytes: Bytes = hdr_block_frag.into();
                buf.put_u8(pad_length.try_into().unwrap());
                buf.put_u32(if e {
                    stream_dep | 0x8000_0000
                } else {
                    stream_dep
                });
                buf.put_u8(weight);
                buf.put(hdr_block_frag_bytes);
                buf.put_bytes(0, pad_length);
            }
            Self::Priority {
                e,
                stream_pri,
                weight,
            } => {
                buf.put_u32(if e {
                    stream_pri | 0x8000_0000
                } else {
                    stream_pri
                });
                buf.put_u8(weight.try_into().unwrap());
            }
            Self::RstStream { error_code } => {
                buf.put_u32(error_code as u32);
            }
            Self::Settings { identifier, value } => {
                buf.put_u16(identifier as u16);
                buf.put_u32(value);
            }
            Self::PushPromise {
                pad_length,
                promised_stream_id,
                hdr_block_frag,
            } => {
                let hdr_block_frag_bytes: Bytes = hdr_block_frag.into();
                buf.put_u8(pad_length.try_into().unwrap());
                buf.put_u32(promised_stream_id & 0x7fff_ffff);
                buf.put(hdr_block_frag_bytes);
                buf.put_bytes(0, pad_length);
            }
            Self::Ping { data, .. } => {
                buf.put(data);
            }
            Self::GoAway {
                last_stream_id,
                error_code,
                additional_debug_data,
            } => {
                buf.put_u32(last_stream_id & 0x7fff_ffff);
                buf.put_u32(error_code as u32);
                buf.put(additional_debug_data);
            }
            Self::WindowUpdate {
                window_size_increment,
            } => {
                buf.put_u32(window_size_increment & 0x7fff_ffff);
            }
            Self::Continuation { hdr_block_frag } => {
                let hdr_block_frag_bytes: Bytes = hdr_block_frag.into();
                buf.put(hdr_block_frag_bytes);
            }
        }
    }
}

pub struct Frame {
    header: FrameHeader,
    payload: FrameBody,
}
impl Frame {
    pub fn new(stream_id: u32, payload: FrameBody, flags: u8) -> Self {
        let body_size = payload.size();
        Self {
            header: FrameHeader {
                length: body_size,
                frame_type: payload.to_id(),
                flags,
                stream_id,
            },
            payload,
        }
    }

    pub fn validate(self) -> Result<(), &'static str> {
        // TODO: Implement this
        Ok(())
    }

    pub fn read_from_buf(buf_reader: BufReader<TcpStream>) -> Frame {
        // Read frame header
    }
}
impl Into<Bytes> for Frame {
    fn into(self) -> Bytes {
        let mut buf = BytesMut::with_capacity(FRAME_HDR_SIZE + self.header.length);
        self.header.put_buf(buf);
        self.payload.put_buf(buf);
        buf.into()
    }
}

#[repr(u32)]
enum ErrorCodes {
    NoError = 0x0,
    ProtocolError = 0x1,
    InternalError = 0x2,
    FlowControlError = 0x3,
    SettingsTimeout = 0x4,
    StreamClosed = 0x5,
    FrameSizeError = 0x6,
    RefusedStream = 0x7,
    Cancel = 0x8,
    CompressionError = 0x9,
    ConnectError = 0xa,
    EnhanceYourCalm = 0xb,
    InadequateSecurity = 0xc,
    HTTP1_1Required = 0xd,
}
