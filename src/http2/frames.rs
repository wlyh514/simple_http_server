
use std::{net::TcpStream, io::BufReader};

use bitflags::bitflags;
use bytes::Bytes;

use crate::http::HeadersMap;
use super::connection::Settings;

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

#[repr(u8)]
pub enum Frame {
    Data {
        flags: DataFlags,
        pad_length: u8,
        data: Bytes,
    } = 0x0,
    Headers {
        flags: HeadersFlags,
        pad_length: u8,
        e: bool,
        stream_dep: u32,
        weight: u8,
        hdr_block_frag: HeadersMap,
    } = 0x1,
    Priority {
        e: bool,
        stream_pri: u32,
        weight: u8,
    } = 0x2,
    RstStream {
        error_code: u32,
    } = 0x3,
    Settings {
        flags: SettingsFlags,
        identifier: Settings,
        value: u32,
    } = 0x4,
    PushPromise {
        flags: PushPromiseFlags,
        pad_length: u8,
        r: bool,
        promised_stream_id: u32,
        hdr_block_frag: HeadersMap,
    } = 0x5,
    Ping {
        flags: PingFlags,
        data: Bytes,
    } = 0x6,
    GoAway {
        r: bool,
        last_stream_id: u32,
        error_code: u32,
        additional_debug_data: Bytes,
    } = 0x7,
    WindowUpdate {
        r: bool,
        window_size_increment: u32,
    } = 0x8,
    Continuation {
        flags: ContinuationFlags,
        hdr_block_frag: HeadersMap,
    } = 0x9,
}
impl Frame {
    pub fn read_from_buf(buf_reader: BufReader<TcpStream>) -> Frame {
        // TODO: Implement this
    }
}
impl Into<Bytes> for Frame {
    fn into(self) -> Bytes {
        
    }
}

#[repr(u32)]
enum ErorCodes {
    NoError             = 0x0,
    ProtocolError       = 0x1,
    InternalError       = 0x2, 
    FlowControlError    = 0x3, 
    SettingsTimeout     = 0x4, 
    StreamClosed        = 0x5,
    FrameSizeError      = 0x6,
    RefusedStream       = 0x7, 
    Cancel              = 0x8, 
    CompressionError    = 0x9, 
    ConnectError        = 0xa,
    EnhanceYourCalm     = 0xb, 
    InadequateSecurity  = 0xc, 
    HTTP1_1Required     = 0xd,
}