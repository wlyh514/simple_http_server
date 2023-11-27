use std::io::Read;
use ::std::{io::BufReader, net::TcpStream};

use ::bitflags::bitflags;
use ::bytes::{BufMut, Bytes, BytesMut, Buf};
use ::num_enum::{TryFromPrimitive, IntoPrimitive};

use super::connection::SettingsIdentifier;

bitflags! {
    pub struct DataFlags: u8 {
        const END_STREAM = 0x1;
        const PADDED = 0x8;
    }

    pub struct HeadersFlags: u8 {
        const END_STREAM = 0x1;
        const END_HEADERS = 0x4;
        const PADDED = 0x8;
        const PRIORITY = 0x20;
    }

    pub struct SettingsFlags: u8 {
        const ACK = 0x1;
    }

    pub struct PushPromiseFlags: u8 {
        const END_HEADERS = 0x4;
        const PADDED = 0x8;
    }

    pub struct PingFlags: u8 {
        const ACK = 0x1;
    }

    pub struct ContinuationFlags: u8 {
        const END_HEADERS = 0x4;
    }
}

const FRAME_HDR_SIZE: usize = 24 + 8 + 8 + 32;
/// See RFC7540 section 4
struct FrameHeader {
    pub length: usize,
    frame_type: u8,
    pub flags: u8,
    pub stream_id: u32,
}
impl FrameHeader {
    fn put_buf(self, buf: &mut BytesMut) {
        // Write length as u24 in big endian
        buf.put_u8(((self.length >> 16) & 0xff) as u8);
        buf.put_u8(((self.length >> 8) & 0xff) as u8);
        buf.put_u8((self.length & 0xff) as u8);

        buf.put_u8(self.frame_type);
        buf.put_u8(self.flags);
        buf.put_u32(self.stream_id & 0x7fff_ffff);
    }
}
impl TryFrom<Bytes> for FrameHeader {
    type Error = error::HeaderSerializationError;

    fn try_from(mut buf: Bytes) -> Result<Self, Self::Error> {
        if buf.len() > 9 {
            return Err(Self::Error::BufferTooSmall);
        }
        let length: usize = ((buf.get_u8() as usize) << 16) | ((buf.get_u8() as usize) << 8) | (buf.get_u8() as usize);
        let frame_type = buf.get_u8(); 
        let flags = buf.get_u8();
        let stream_id = buf.get_u32() & 0x7fff_ffff;
        
        if frame_type > 0x9 {
            Err(Self::Error::UnknownFrameType(frame_type))
        } else {
            Ok(Self {length, frame_type, flags, stream_id})
        }
    }
}

pub struct SettingParam {
    pub identifier: SettingsIdentifier, 
    pub value: u32
}

/// See RFC7540 section 6
pub enum FrameBody {
    Data {
        pad_length: usize,
        data: Bytes,
    },
    Headers {
        pad_length: usize,
        e: bool,
        stream_dep: u32,
        weight: u8,
        hdr_block_frag: Bytes,
    },
    Priority {
        e: bool,
        stream_dep: u32,
        weight: u8,
    },
    RstStream {
        error_code: ErrorCode,
    },
    Settings (Vec<SettingParam>),
    PushPromise {
        pad_length: usize,
        promised_stream_id: u32,
        hdr_block_frag: Bytes,
    },
    Ping {
        data: Bytes,
    },
    GoAway {
        last_stream_id: u32,
        error_code: ErrorCode,
        additional_debug_data: Bytes,
    },
    WindowUpdate {
        window_size_increment: u32,
    },
    Continuation {
        hdr_block_frag: Bytes,
    },
}
impl FrameBody {
    fn get_id(&self) -> u8 {
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

    /// Errors if hdr_block_frag failed to compress
    fn size(&self) -> usize {
        match self {
            Self::Data { pad_length, data } => {
                8 + data.len() + pad_length
            },
            Self::Headers { pad_length, hdr_block_frag, ..} => {
                8 + 32 + 8 + hdr_block_frag.len() + pad_length
            },
            Self::Priority { .. } => {
                32 + 8
            },
            Self::RstStream { .. } => {
                32
            },
            Self::Settings { .. } => {
                16 + 32
            },
            Self::PushPromise { pad_length, hdr_block_frag, .. } => {
                8 + 32 + hdr_block_frag.len() + pad_length
            },
            Self::Ping { .. } => {
                64
            },
            Self::GoAway { additional_debug_data, .. } => {
                32 + 32 + additional_debug_data.len()
            },
            Self::WindowUpdate { .. } => {
                32
            },
            Self::Continuation { hdr_block_frag } => {
                hdr_block_frag.len()
            }
        }
    }

    /// Serialization
    fn try_put_buf(self, buf: &mut BytesMut) -> Result<(), &'static str> {
        match self {
            Self::Data { pad_length, data } => {
                buf.put_u8(pad_length.try_into().unwrap());
                buf.put(data);
                buf.put_bytes(0, pad_length);
                Ok(())
            }
            Self::Headers {
                pad_length,
                e,
                stream_dep,
                weight,
                hdr_block_frag,
            } => {
                let hdr_block_frag_bytes: Bytes = hdr_block_frag.try_into().map_err(|_| "Error compressing header")?;
                buf.put_u8(pad_length.try_into().unwrap());
                buf.put_u32(if e {
                    stream_dep | 0x8000_0000
                } else {
                    stream_dep
                });
                buf.put_u8(weight);
                buf.put(hdr_block_frag_bytes);
                buf.put_bytes(0, pad_length);
                Ok(())
            }
            Self::Priority {
                e,
                stream_dep: stream_pri,
                weight,
            } => {
                buf.put_u32(if e {
                    stream_pri | 0x8000_0000
                } else {
                    stream_pri
                });
                buf.put_u8(weight.try_into().unwrap());
                Ok(())
            }
            Self::RstStream { error_code } => {
                buf.put_u32(error_code as u32);
                Ok(())
            }
            Self::Settings (params) => {
                for param in params {
                    buf.put_u16(param.identifier as u16);
                    buf.put_u32(param.value);
                }
                Ok(())
            }
            Self::PushPromise {
                pad_length,
                promised_stream_id,
                hdr_block_frag,
            } => {
                let hdr_block_frag_bytes: Bytes = hdr_block_frag.try_into().map_err(|_| "Error compressing header")?;
                buf.put_u8(pad_length.try_into().unwrap());
                buf.put_u32(promised_stream_id & 0x7fff_ffff);
                buf.put(hdr_block_frag_bytes);
                buf.put_bytes(0, pad_length);
                Ok(())
            }
            Self::Ping { data, .. } => {
                buf.put(data);
                Ok(())
            }
            Self::GoAway {
                last_stream_id,
                error_code,
                additional_debug_data,
            } => {
                buf.put_u32(last_stream_id & 0x7fff_ffff);
                buf.put_u32(error_code as u32);
                buf.put(additional_debug_data);
                Ok(())
            }
            Self::WindowUpdate {
                window_size_increment,
            } => {
                buf.put_u32(window_size_increment & 0x7fff_ffff);
                Ok(())
            }
            Self::Continuation { hdr_block_frag } => {
                let hdr_block_frag_bytes: Bytes = hdr_block_frag.try_into().map_err(|_| "Error compressing header")?;
                buf.put(hdr_block_frag_bytes);
                Ok(())
            }
        }
    }

    /// Deserialization
    fn try_from_buf(mut buf: Bytes, hdr: &FrameHeader) -> Result<Self, error::BodySerializationError> {
        match hdr.frame_type {
            0x0 => {
                let pad_length: usize = buf.get_u8().into();
                let data_length = hdr.length - 8 - pad_length;
                let data = buf.slice(..data_length);
                Ok(Self::Data { pad_length, data })
            }, 
            0x1 => {
                let pad_length: usize = buf.get_u8().into(); 

                let stream_dep = buf.get_u32();
                let e: bool = (stream_dep & 0x8000_0000) > 0;
                let stream_dep = stream_dep * 0x7fff_ffff;

                let weight = buf.get_u8();

                let hdr_block_frag_len = hdr.length - 8 - 32 - 8 - pad_length;
                let hdr_block_frag = buf.slice(..hdr_block_frag_len);
                Ok(Self::Headers { pad_length, e, stream_dep, weight, hdr_block_frag })
            }, 
            0x2 => {
                let stream_dep = buf.get_u32();
                let e: bool = (stream_dep & 0x8000_0000) > 0;
                let stream_dep = stream_dep * 0x7fff_ffff;

                let weight = buf.get_u8();
                Ok(Self::Priority { e, stream_dep, weight })
            },
            0x3 => {
                let error_code = buf.get_u32();
                match error_code.try_into() {
                    Ok(error_code) => Ok(Self::RstStream { error_code }),
                    Err(_) => Err(error::BodySerializationError::UnknownErrorCode(error_code)),
                }
            },
            0x4 => {
                let mut remaining_size: usize = hdr.length;
                let mut setting_params: Vec<SettingParam> = vec![];
                while remaining_size >= 48 {
                    let identifier = buf.get_u16();
                    let identifier: SettingsIdentifier = identifier.try_into()
                        .map_err(|_| error::BodySerializationError::UnknownSettingsIdentifier(identifier))?;
                    let value = buf.get_u32();

                    setting_params.push(SettingParam { identifier, value });
                    remaining_size -= 48;
                }

                Ok(Self::Settings (setting_params))
            },
            0x5 => {
                let pad_length: usize = buf.get_u8().into(); 
                let promised_stream_id = buf.get_u32() & 0x7fff_ffff;
                let hdr_block_frag = buf.slice(..(hdr.length - 8 - 32 - pad_length));
                Ok(Self::PushPromise { pad_length, promised_stream_id, hdr_block_frag})
            },
            0x6 => {
                let data = buf.slice(..64);
                Ok(Self::Ping { data })
            },
            0x7 => {
                let last_stream_id = buf.get_u32() & 0x7fff_ffff;
                let error_code = buf.get_u32();
                let error_code: ErrorCode = error_code.try_into()
                    .map_err(|_| error::BodySerializationError::UnknownErrorCode(error_code))?;
                let additional_debug_data = buf.slice(..(hdr.length - 32 - 32));
                Ok(Self::GoAway { last_stream_id, error_code, additional_debug_data })
            },
            0x8 => {
                let window_size_increment = buf.get_u32() & 0x7fff_ffff;
                Ok(Self::WindowUpdate { window_size_increment })
            },
            0x9 => {
                let hdr_block_frag = buf.slice(..hdr.length);
                Ok(Self::Continuation { hdr_block_frag })
            },
            _ => panic!("Header frame_type should be validated at this point. ")
        }
    }
}

pub struct Frame {
    pub header: FrameHeader,
    pub payload: FrameBody,
}
impl Frame {
    pub fn new(stream_id: u32, flags: u8, payload: FrameBody) -> Self{
        let body_size = payload.size();
        Self {
            header: FrameHeader {
                length: body_size,
                frame_type: payload.get_id(),
                flags,
                stream_id,
            },
            payload,
        }
    }

    /// A frame must be verified before serialized and sent. 
    pub fn validate(&self) -> Result<(), &'static str> {
        let payload_len = self.payload.size();
        if payload_len != self.header.length {
            return Err("Incorrect FrameHeader.length")
        }

        // TODO: Check for unsupported header

        // TODO: Get a better name for this fn
        Ok(())
    }

    /// Deserialization
    pub fn try_read_from_buf(mut buf_reader: BufReader<TcpStream>) -> Result<Frame, error::SerializationError> {
        let mut header_buf = BytesMut::with_capacity(9);
        // Read frame header
        buf_reader.read_exact(&mut header_buf);
        let header_buf: Bytes = header_buf.into();
        let header = FrameHeader::try_from(header_buf)
            .map_err(|err| error::SerializationError::Header(err))?;

        let mut payload_buf = BytesMut::with_capacity(header.length);
        buf_reader.read_exact(&mut payload_buf);
        let payload_buf: Bytes = payload_buf.into();
        
        let payload = FrameBody::try_from_buf(payload_buf, &header)
            .map_err(|err| error::SerializationError::Body(err))?;
        Ok(Self { header, payload })
    }
}
/// Serialization
impl TryInto<Bytes> for Frame {
    type Error = &'static str;

    fn try_into(self) -> Result<Bytes, Self::Error> {
        let mut buf = BytesMut::with_capacity(FRAME_HDR_SIZE + self.header.length);
        self.header.put_buf(&mut buf);
        self.payload.try_put_buf(&mut buf)?;
        Ok(buf.into())
    }
}

#[derive(Debug, Eq, PartialEq, TryFromPrimitive, IntoPrimitive, Clone)]
#[repr(u32)]
pub enum ErrorCode {
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

mod error {
    #[derive(Debug)]
    pub enum HeaderSerializationError {
        BufferTooSmall, 
        UnknownFrameType(u8),
    }

    #[derive(Debug)]
    pub enum BodySerializationError {
        UnknownErrorCode(u32), 
        UnknownSettingsIdentifier(u16),
    }

    #[derive(Debug)]
    pub enum SerializationError {
        Header(HeaderSerializationError),
        Body(BodySerializationError)
    }
}