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

const FRAME_HDR_SIZE: usize = 3 + 1 + 1 + 4;
/// See RFC7540 section 4
#[derive(Clone, Debug)]
pub struct FrameHeader {
    pub length: usize,
    frame_type: u8,
    pub flags: u8,
    pub stream_id: u32,
}
impl FrameHeader {
    fn put_buf(&self, buf: &mut BytesMut) {
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
    type Error = error::HeaderDeserializationError;

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

#[derive(Clone, Debug)]
pub struct SettingParam {
    pub identifier: SettingsIdentifier, 
    pub value: u32
}

#[derive(Clone, Debug)]
pub struct HeadersBodyPriority {
    e: bool, 
    stream_dep: u32, 
    weight: u8,
}

/// See RFC7540 section 6
#[derive(Clone, Debug)]
pub enum FrameBody {
    Data {
        pad_length: Option<usize>,
        data: Bytes,
    },
    Headers {
        pad_length: Option<usize>,
        priority: Option<HeadersBodyPriority>,
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
        pad_length: Option<usize>,
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
                data.len() + pad_length.map_or(0, |n| n + 1)
            },
            Self::Headers { pad_length, hdr_block_frag, priority } => {
                pad_length.map_or(0, |n| n + 1) + hdr_block_frag.len() + priority.as_ref().map_or(0, |_| 5)
            },
            Self::Priority { .. } => {
                4 + 1
            },
            Self::RstStream { .. } => {
                4
            },
            Self::Settings (settings) => {
                settings.len() * 6
            },
            Self::PushPromise { pad_length, hdr_block_frag, .. } => {
                pad_length.map_or(0, |n| n + 1) + 4 + hdr_block_frag.len()
            },
            Self::Ping { .. } => {
                8
            },
            Self::GoAway { additional_debug_data, .. } => {
                4 + 4 + additional_debug_data.len()
            },
            Self::WindowUpdate { .. } => {
                4
            },
            Self::Continuation { hdr_block_frag } => {
                hdr_block_frag.len()
            }
        }
    }

    /// Serialization
    fn try_put_buf(&self, buf: &mut BytesMut) -> Result<(), &'static str> {
        match self {
            Self::Data { pad_length, data } => {
                let pad_length = match *pad_length {
                    Some(pad_length) => {
                        let pad_length_u8: u8 = pad_length.try_into().unwrap();
                        buf.put_u8(pad_length_u8);
                        pad_length
                    },
                    None => { 0 }
                };

                buf.put(data.as_ref());
                buf.put_bytes(0, pad_length);
                Ok(())
            }
            Self::Headers {
                pad_length,
                priority,
                hdr_block_frag,
            } => {
                let hdr_block_frag_bytes: Bytes = (*hdr_block_frag).clone().try_into().map_err(|_| "Error compressing header")?;

                let pad_length = match pad_length {
                    Some(pad_length) => {
                        buf.put_u8(*pad_length as u8);
                        *pad_length
                    },
                    None => { 0 }
                };
                
                match priority {
                    Some(priority) => {
                        buf.put_u32(if priority.e { priority.stream_dep | 0x8000_0000 } else { priority.stream_dep });
                        buf.put_u8(priority.weight);
                    }, 
                    None => {}
                };

                buf.put(hdr_block_frag_bytes.as_ref());
                buf.put_bytes(0, pad_length);
                Ok(())
            }
            Self::Priority {
                e,
                stream_dep: stream_pri,
                weight,
            } => {
                buf.put_u32(if *e {
                    stream_pri | 0x8000_0000
                } else {
                    *stream_pri
                });
                buf.put_u8((*weight).try_into().unwrap());
                Ok(())
            }
            Self::RstStream { error_code } => {
                buf.put_u32(((*error_code).clone()) as u32);
                Ok(())
            }
            Self::Settings (params) => {
                for param in params {
                    buf.put_u16(param.identifier.clone() as u16);
                    buf.put_u32(param.value);
                }
                Ok(())
            }
            Self::PushPromise {
                pad_length,
                promised_stream_id,
                hdr_block_frag,
            } => {
                let hdr_block_frag_bytes: Bytes = (*hdr_block_frag).clone().try_into().map_err(|_| "Error compressing header")?;
                
                let pad_length = match pad_length {
                    Some(pad_length) => {
                        buf.put_u8(*pad_length as u8);
                        *pad_length
                    },
                    None => {
                        0
                    }
                };

                buf.put_u32(promised_stream_id & 0x7fff_ffff);
                buf.put(hdr_block_frag_bytes.as_ref());
                buf.put_bytes(0, pad_length);
                Ok(())
            }
            Self::Ping { data, .. } => {
                buf.put(data.as_ref());
                Ok(())
            }
            Self::GoAway {
                last_stream_id,
                error_code,
                additional_debug_data,
            } => {
                buf.put_u32(last_stream_id & 0x7fff_ffff);
                buf.put_u32((*error_code).clone() as u32);
                buf.put(additional_debug_data.as_ref());
                Ok(())
            }
            Self::WindowUpdate {
                window_size_increment,
            } => {
                buf.put_u32(window_size_increment & 0x7fff_ffff);
                Ok(())
            }
            Self::Continuation { hdr_block_frag } => {
                let hdr_block_frag_bytes: Bytes = (*hdr_block_frag).clone().try_into().map_err(|_| "Error compressing header")?;
                buf.put(hdr_block_frag_bytes.as_ref());
                Ok(())
            }
        }
    }

    /// Deserialization
    fn try_from_buf(mut buf: Bytes, hdr: &FrameHeader) -> Result<Self, error::BodyDeserializationError> {
        let flags = hdr.flags;
        match hdr.frame_type {
            0x0 => {
                let pad_length = if DataFlags::from_bits_retain(flags).contains(DataFlags::PADDED) {
                    Some(buf.get_u8().into())
                } else {
                    None
                };

                let data_length = hdr.length - 1 - pad_length.unwrap_or(0);
                let data = buf.slice(..data_length);
                Ok(Self::Data { pad_length, data })
            }, 
            0x1 => {
                let flags = HeadersFlags::from_bits_retain(flags);
                let mut hdr_block_frag_len: usize = hdr.length;

                let pad_length = if flags.contains(HeadersFlags::PADDED) {
                    let pad_length: usize = buf.get_u8().into();
                    hdr_block_frag_len -= 1 + pad_length;
                    Some(pad_length)
                } else {
                    None
                };

                let priority: Option<HeadersBodyPriority> = if flags.contains(HeadersFlags::PRIORITY) {
                    let stream_dep_with_e = buf.get_u32();
                    hdr_block_frag_len -= 4 + 1;
                    Some(HeadersBodyPriority { 
                        e: (stream_dep_with_e & 0x8000_0000) > 0, 
                        stream_dep: stream_dep_with_e & 0x7fff_ffff, 
                        weight: buf.get_u8() 
                    })
                } else {
                    None
                };

                let hdr_block_frag = buf.slice(..hdr_block_frag_len);
                Ok(Self::Headers { pad_length, priority, hdr_block_frag })
            }, 
            0x2 => {
                let stream_dep = buf.get_u32();
                let e: bool = (stream_dep & 0x8000_0000) > 0;
                let stream_dep = stream_dep & 0x7fff_ffff;

                let weight = buf.get_u8();
                Ok(Self::Priority { e, stream_dep, weight })
            },
            0x3 => {
                let error_code = buf.get_u32();
                match error_code.try_into() {
                    Ok(error_code) => Ok(Self::RstStream { error_code }),
                    Err(_) => Err(error::BodyDeserializationError::UnknownErrorCode(error_code)),
                }
            },
            0x4 => {
                let mut remaining_size: usize = hdr.length;
                let mut setting_params: Vec<SettingParam> = vec![];
                while remaining_size >= 6 {
                    let identifier = buf.get_u16();
                    let identifier: SettingsIdentifier = identifier.try_into()
                        .map_err(|_| error::BodyDeserializationError::UnknownSettingsIdentifier(identifier))?;
                    let value = buf.get_u32();

                    setting_params.push(SettingParam { identifier, value });
                    remaining_size -= 6;
                }

                Ok(Self::Settings (setting_params))
            },
            0x5 => {
                let mut hdr_block_frag_len = hdr.length - 4;
                let pad_length = if PushPromiseFlags::from_bits_retain(flags).contains(PushPromiseFlags::PADDED) {
                    let pad_length: usize = buf.get_u8().into();
                    hdr_block_frag_len -= 1 + pad_length;
                    Some(pad_length)
                } else {
                    None
                };
 
                let promised_stream_id = buf.get_u32() & 0x7fff_ffff;
                let hdr_block_frag = buf.slice(..hdr_block_frag_len);
                Ok(Self::PushPromise { pad_length, promised_stream_id, hdr_block_frag})
            },
            0x6 => {
                let data = buf.slice(..8);
                Ok(Self::Ping { data })
            },
            0x7 => {
                let last_stream_id = buf.get_u32() & 0x7fff_ffff;
                let error_code = buf.get_u32();
                let error_code: ErrorCode = error_code.try_into()
                    .map_err(|_| error::BodyDeserializationError::UnknownErrorCode(error_code))?;
                let additional_debug_data = buf.slice(..(hdr.length - 4 - 4));
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

#[derive(Clone, Debug)]
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
    pub fn is_valid(&self) -> Result<(), &'static str> {
        let payload_len = self.payload.size();
        if payload_len != self.header.length {
            return Err("Incorrect FrameHeader.length")
        }

        // TODO: Check for unsupported header
        Ok(())
    }

    /// Deserialization
    pub fn try_read_from_buf(buf_reader: &mut BufReader<TcpStream>) -> Result<Frame, error::DeserializationError> {
        let mut header_buf = BytesMut::zeroed(9);
        // Read frame header
        buf_reader.read_exact(header_buf.as_mut()).map_err(|_| error::DeserializationError::BufReaderError)?;
        let header_buf: Bytes = Bytes::copy_from_slice(&header_buf);
        
        let header = FrameHeader::try_from(header_buf)
            .map_err(|err| error::DeserializationError::Header(err))?;

        let mut payload_buf: Vec<u8> = vec![0; header.length];
        buf_reader.read_exact(payload_buf.as_mut_slice()).map_err(|_| error::DeserializationError::BufReaderError)?;
        let payload_buf: Bytes = payload_buf.into();
        
        let payload = FrameBody::try_from_buf(payload_buf, &header)
            .map_err(|err| error::DeserializationError::Body(err))?;

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

pub mod error {
    #[derive(Debug)]
    pub enum HeaderDeserializationError {
        BufferTooSmall, 
        UnknownFrameType(u8),
    }

    #[derive(Debug)]
    pub enum BodyDeserializationError {
        UnknownErrorCode(u32), 
        UnknownSettingsIdentifier(u16),
    }

    #[derive(Debug)]
    pub enum DeserializationError {
        Header(HeaderDeserializationError),
        Body(BodyDeserializationError),
        BufReaderError,
    }
}