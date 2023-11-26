use bytes::Bytes;
use ::num_enum::{TryFromPrimitive, IntoPrimitive};

use crate::http::{ReqHandlerFn, HTTPRequest, HTTPResponse, hdr_map_size, HeaderVal};

use super::{frames::{Frame, SettingParam, FrameBody, HeadersFlags, ContinuationFlags, DataFlags, ErrorCode, SettingsFlags}, stream::{Stream, compress_header}};
use std::io::{Write, BufReader};
use ::std::{collections::HashMap, net::TcpStream, thread, sync::{Arc, Mutex, mpsc}};

enum ResponseQueueError {
    Terminated
}
struct ResponseQueueRx(mpsc::Receiver<Frame>);
#[derive(Clone)]
struct ResponseQueueTx(mpsc::Sender<Frame>);

impl ResponseQueueRx {
    pub fn new(rx: mpsc::Receiver<Frame>) -> Self {
        Self(rx)
    }

    pub fn pop(&self) -> Result<Frame, ResponseQueueError> {
        self.0.recv().map_err(|_| ResponseQueueError::Terminated)
    }
}

impl ResponseQueueTx {
    pub fn new(tx: mpsc::Sender<Frame>) -> Self {
        Self(tx)
    }

    pub fn push(&self, frame: Frame) {
        match self.0.send(frame) { _ => () }
    }
}

pub struct Connection<T: ReqHandlerFn + Sync> {
    tcp_stream: TcpStream,
    peer_settings: Arc<Mutex<SettingsMap>>,
    stream_counter: u32,
    active_streams: Arc<Mutex<HashMap<u32, Stream>>>,
    handler: T
}

impl<T: ReqHandlerFn + Sync> Connection<T> {
    pub fn new(tcp_stream: TcpStream, handler: T, peer_settings_params: Vec<SettingParam>) -> Connection<T> {
        Connection {
            tcp_stream,
            peer_settings: Arc::new(Mutex::new(SettingsMap::from(peer_settings_params))),
            stream_counter: 1,
            active_streams: Arc::new(Mutex::new(HashMap::new())),
            handler,
        }
    }

    fn new_stream(&mut self) -> Stream {
        // TODO: Implement this
    }

    /// Convert a http response into frames to be sent. 
    fn make_frames(&self, stream_id: u32, mut response: HTTPResponse) -> Result<Vec<Frame>, ()> {
        let stream = self.active_streams.lock().unwrap().get(&stream_id).map_or(Err(()), |val| Ok(val))?;
        
        // Write response pseudoheaders
        let status_code: u32 = response.status as u32;
        response.headers.insert(":status".to_string(), HeaderVal::Single(status_code.to_string()));

        let header_bytes = compress_header(response.headers);
        
        // Load relavent settings
        let mut hdr_partition_size: usize; 
        let mut body_partition_size: usize;
        let mut max_hdr_size: usize;
        {
            let settings = self.peer_settings.lock().unwrap(); 
            let frame_partition_size = settings.get(SettingsIdentifier::MaxFrameSize).unwrap() as usize;
            hdr_partition_size = settings.get(SettingsIdentifier::HeaderTableSize).unwrap() as usize;
            max_hdr_size = settings.get(SettingsIdentifier::MaxHeaderListSize).unwrap() as usize;

            if hdr_partition_size > frame_partition_size {
                hdr_partition_size = frame_partition_size;
            }
            body_partition_size = frame_partition_size - 8;
        }
        if max_hdr_size < hdr_map_size(response.headers) {
            Err(())
        } else {
            let mut frames: Vec<Frame> = vec![]; 
            let mut hdr_remaining_bytes = header_bytes.len();
            
            // Header frames
            while hdr_remaining_bytes > 0 {
                let mut bytes_taken = usize::min(hdr_remaining_bytes, hdr_partition_size);

                // First header frame
                if hdr_remaining_bytes == header_bytes.len() {
                    let mut flags = HeadersFlags::from_bits_retain(0);  

                    let first_frag_size = hdr_partition_size - 8 - 32 - 8;
                    bytes_taken = usize::min(bytes_taken, first_frag_size);
                    
                    if hdr_remaining_bytes < first_frag_size {
                        if response.body.is_none() {
                            flags &= HeadersFlags::END_STREAM;
                        } else {
                            flags &= HeadersFlags::END_HEADERS;
                        }
                    }

                    frames.push(Frame::new(stream.id, flags.bits(), FrameBody::Headers { pad_length: 0, e: false, stream_dep: 0, weight: 0, hdr_block_frag: header_bytes.slice(..bytes_taken) }));
                } else {
                    let mut flags = ContinuationFlags::from_bits_retain(0);

                    if hdr_remaining_bytes < hdr_partition_size {
                        flags &= ContinuationFlags::END_HEADERS;
                    }

                    frames.push(Frame::new(stream_id, flags.bits(), FrameBody::Continuation { hdr_block_frag: header_bytes.slice(..bytes_taken)}));
                }

                hdr_remaining_bytes -= bytes_taken;
            }

            // Body frames
            if let Some(body_bytes) = response.body {
                let mut body_remaining_bytes = body_bytes.len();
                while body_remaining_bytes > 0 {
                    let bytes_taken = usize::min(body_remaining_bytes, body_partition_size);

                    let mut flags = DataFlags::from_bits_retain(0);
                    if body_remaining_bytes < body_partition_size {
                        flags &= DataFlags::END_STREAM;
                    }

                    frames.push(Frame::new(stream_id, flags.bits(), FrameBody::Data { pad_length: 0, data: body_bytes.slice(..bytes_taken) }));

                    body_remaining_bytes -= bytes_taken;
                }
            }

            Ok(frames)
        }
    }

    /// Send a frame and update the frame's state machine accordingly. 
    fn send_frame(&self, frame: Frame, mut stream: Stream) -> Result<(), ()> {
        stream.send(frame);
        let frame_bytes: Bytes = frame.try_into().map_err(|_| ())?;
        self.tcp_stream.write_all(&frame_bytes);

        Ok(())
    }

    fn handle_request(&self, req: HTTPRequest, stream_id: u32, queue_tx: ResponseQueueTx) {
        let resp = (self.handler)(req);
        match self.make_frames(stream_id, resp) {
            Ok(frames) => {
                for frame in frames {
                    queue_tx.push(frame)
                }
            },
            Err(_) => ()
        }
    }

    fn run_rx(&self, queue_tx: ResponseQueueTx) {
        loop {
            let frame: Option<Frame> = match self.tcp_stream.try_clone() {
                Ok(tcp_stream) => {
                    let tcp_reader = BufReader::new(tcp_stream);
                    Frame::try_read_from_buf(tcp_reader).ok()
                },
                _ => None
            };

            let frame = match frame {
                Some(frame) => frame,
                None => {
                    self.close_with_error(ErrorCode::ProtocolError, 0, queue_tx.clone());
                    break;
                }
            };

            let new_stream_required = false;        // TODO: Implement this
            let mut stream: Stream = match new_stream_required {
                true => {
                    let new_stream = self.new_stream();
                    self.active_streams.lock().unwrap().insert(new_stream.id, new_stream);
                    new_stream
                }, 
                false => {
                    *self.active_streams.lock().unwrap().get(&114514).unwrap()
                },
            };

            let resp_frame: Result<Option<Frame>, ErrorCode>  = match frame.payload {
                // See section 6.5
                FrameBody::Settings(settings) => {
                    let flags = SettingsFlags::from_bits_retain(frame.header.flags);

                    // If settings frame is sent over stream other than 0x0, connection error
                    if frame.header.stream_id != 0x0 {
                        Err(ErrorCode::ProtocolError)
                    } else {
                        if flags.contains(SettingsFlags::ACK) {
                            if frame.header.length != 0 {
                                Err(ErrorCode::FrameSizeError)
                            } else {
                                Ok(None)
                            }
                        } else {
                            // Update self.settings
                            self.peer_settings.lock().unwrap().update_with_vec(settings);
                            
                            // Send ACK
                            let flag = SettingsFlags::ACK;
                            Ok(Some(Frame::new(0, flag.bits(), FrameBody::Settings(vec![]))))
                        }
                    }
                },
                // Kill connection
                FrameBody::GoAway { last_stream_id, error_code, additional_debug_data } => {
                    break;
                }
                _ => Ok(None), 
            }; 

            match resp_frame {
                Ok(Some(resp_frame)) => {
                    queue_tx.push(resp_frame);
                },
                Err(error_code) => {
                    self.close_with_error(error_code, frame.header.stream_id, queue_tx);
                    break;
                },
                _ => {},
            }

            match stream.recv(frame) {
                Ok(Some(req)) => {
                    let tx = queue_tx.clone();
                    thread::spawn(|| self.handle_request(req, 114514, tx));
                }, 
                Err(err) => {
                    // Close connection if connection error
                    break;
                },
                _ => {}
            }
        }
    }

    fn run_tx(&self, queue_rx: ResponseQueueRx) {
        loop {
            let frame = match queue_rx.pop() {
                Ok(tagged_resp) => { tagged_resp },
                Err(_) => { break; }
            };
            if let Some(stream) = self.active_streams.lock().unwrap().get(&frame.header.stream_id) {
                self.send_frame(frame, *stream);
            }
        }
    }

    fn close_with_error(&mut self, error_code: ErrorCode, last_stream_id: u32, queue_tx: ResponseQueueTx) {
        let frame: Frame = Frame::new(0, 0, FrameBody::GoAway { last_stream_id, error_code, additional_debug_data: Bytes::new() });
        queue_tx.push(frame);
    }

    pub fn run(&self) {
        let (tx, rx) = mpsc::channel();
        let queue_tx = ResponseQueueTx::new(tx);
        let queue_rx = ResponseQueueRx::new(rx);
        let rx_handle = thread::spawn(move || self.run_rx(queue_tx));
        let tx_handle = thread::spawn(move || self.run_tx(queue_rx));
        rx_handle.join();
        tx_handle.join();
    }
}

/// See RFC7540 section 6.5
#[repr(u16)]
#[derive(TryFromPrimitive, IntoPrimitive, Eq, PartialEq, Debug)]
pub enum SettingsIdentifier {
    HeaderTableSize = 0x1,
    EnablePush = 0x2,
    MaxConcurrentStreams = 0x3,
    InitialWindowSize = 0x4,
    MaxFrameSize = 0x5,
    MaxHeaderListSize = 0x6,
}
impl SettingsIdentifier {
    pub fn is_valid_value(&self, value: u32) -> Result<u32, ErrorCode> {
        const VALID_RANGES: [(SettingsIdentifier, u32, u32, ErrorCode); 3] = [
            (SettingsIdentifier::EnablePush, 0, 1, ErrorCode::ProtocolError),
            (SettingsIdentifier::InitialWindowSize, 0, 2147483647, ErrorCode::FlowControlError),
            (SettingsIdentifier::MaxFrameSize, 16384, 16777215, ErrorCode::ProtocolError)
        ];
        match VALID_RANGES.iter().find(|entry| entry.0 == *self) {
            Some((identifier, min, max, errorCode)) => {
                if min <= &value && &value <= max {
                    Ok(value)
                } else {
                    Err(errorCode.clone())
                }
            }
            None => Ok(value)
        }
    }
}
pub struct SettingsMap (HashMap<u16, u32>);
impl SettingsMap {
    /// Create a SettingsMap with default values
    pub fn default() -> Self {
        let mut settings: HashMap<u16, u32> = HashMap::new();
        let mut settings_map = Self(settings);
        
        // Theses fields should ALWAYS exist within the lifespan of a SettingsMap 
        settings_map.set(SettingsIdentifier::HeaderTableSize, 4096);
        settings_map.set(SettingsIdentifier::EnablePush, 1);
        settings_map.set(SettingsIdentifier::MaxConcurrentStreams, 128);
        settings_map.set(SettingsIdentifier::InitialWindowSize, 65535);
        settings_map.set(SettingsIdentifier::MaxFrameSize, 16384);
        settings_map.set(SettingsIdentifier::MaxHeaderListSize, u32::MAX);

        settings_map
    }

    pub fn set(&mut self, identifier: SettingsIdentifier, value: u32) -> Result<(), ErrorCode> {
        self.0.insert(identifier as u16, identifier.is_valid_value(value)?);
        Ok(())
    }

    pub fn get(&self, identifier: SettingsIdentifier) -> Option<u32> {
        self.0.get(&(identifier as u16)).map(|val| *val)
    }

    pub fn update(&mut self, other: Self) {
        for (key, val) in other.0 {
            self.0.insert(key, val);
        }
    }

    pub fn update_with_vec(&mut self, other: Vec<SettingParam>) {
        for setting in other {
            self.0.insert(setting.identifier as u16, setting.value);
        }
    }
}
impl From<Vec<SettingParam>> for SettingsMap {
    fn from(params: Vec<SettingParam>) -> Self {
        let mut settings: HashMap<u16, u32> = HashMap::new();
        for param in params {
            settings.insert(param.identifier as u16, param.value);
        }
        Self(settings)
    }
}
impl Into<Vec<SettingParam>> for SettingsMap {
    fn into(self) -> Vec<SettingParam> {
        let mut params: Vec<SettingParam> = vec![];
        for (key, value) in self.0 {
            params.push(SettingParam { identifier: key.try_into().unwrap(), value });
        }
        params
    }
}