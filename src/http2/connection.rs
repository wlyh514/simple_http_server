use ::num_enum::{IntoPrimitive, TryFromPrimitive};
use bytes::Bytes;

use crate::{
    http::{hdr_map_size, HTTPRequest, HTTPResponse, HeaderVal, ReqHandlerFn},
    http2::{stream::{StreamState, ReqAssemblerState}, frames::{self, PingFlags}},
};

use super::{
    frames::{
        ContinuationFlags, DataFlags, ErrorCode, Frame, FrameBody, HeadersFlags, SettingParam,
        SettingsFlags,
    },
    stream::{compress_header, Stream},
};
use std::io::BufWriter;
use ::std::{
    collections::HashMap,
    io::{BufReader, Write},
    net::TcpStream,
    sync::{mpsc, Arc, Mutex, atomic::{AtomicU32, Ordering}},
    thread,
};

enum ResponseQueueError {
    Terminated,
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
        match self.0.send(frame) {
            _ => (),
        }
    }
}

#[derive(Clone)]
pub struct Connection<T: ReqHandlerFn + Copy> {
    peer_settings: Arc<Mutex<SettingsMap>>,
    /// max(id of every existed stream) + 1
    stream_counter: Arc<AtomicU32>,
    active_streams: Arc<Mutex<HashMap<u32, Arc<Mutex<Stream>>>>>,
    handler: T,
    id: usize,
}

impl<T: ReqHandlerFn + Copy + 'static> Connection<T> {
    pub fn new(
        handler: T,
        peer_settings_params: Vec<SettingParam>,
        id: usize
    ) -> Connection<T> {
        let mut peer_settings = SettingsMap::default();
        peer_settings.update_with_vec(&peer_settings_params);
        Connection {
            peer_settings: Arc::new(Mutex::new(peer_settings)),
            stream_counter: Arc::new(AtomicU32::new(1)),
            active_streams: Arc::new(Mutex::new(HashMap::new())),
            handler,
            id
        }
    }

    fn new_stream(&self) -> Stream {
        let new_id = self.stream_counter.fetch_add(1,Ordering::SeqCst);
        let new_stream = Stream::new(new_id);

        {
            // Close all idle streams with id smaller than the new stream
            let mut active_streams = self.active_streams.lock().unwrap();
            let mut streams_to_close: Vec<u32> = vec![];
            for (id, stream) in active_streams.iter() {
                let stream = stream.lock().unwrap();
                if stream.state == StreamState::Idle {
                    streams_to_close.push(*id);
                }
            }
            for id in streams_to_close {
                active_streams.remove(&id);
            }
        }

        self.stream_counter.store(new_id, Ordering::SeqCst);
        new_stream
    }

    /// Convert a http response into frames to be sent.
    fn make_frames(&self, stream_id: u32, mut response: HTTPResponse) -> Result<Vec<Frame>, ()> {
        let active_streams = self.active_streams.lock().unwrap();
        let stream = active_streams
            .get(&stream_id)
            .map_or(Err(()), |val| Ok(val))?
            .lock()
            .unwrap();

        // Write response pseudoheaders
        let status_code = response.status as u32;
        response.headers.insert(
            ":status".to_string(),
            HeaderVal::Single(status_code.to_string()),
        );

        let header_bytes = compress_header(&response.headers);

        // Load relavent settings
        let hdr_partition_size: usize;
        let body_partition_size: usize;
        let max_hdr_size: usize;
        {
            let settings = self.peer_settings.lock().unwrap();
            let frame_partition_size =
                settings.get(SettingsIdentifier::MaxFrameSize).unwrap() as usize;
            hdr_partition_size = usize::min(
                settings.get(SettingsIdentifier::HeaderTableSize).unwrap() as usize,
                frame_partition_size,
            );
            max_hdr_size = settings.get(SettingsIdentifier::MaxHeaderListSize).unwrap() as usize;

            body_partition_size = frame_partition_size;
        }
        if max_hdr_size < hdr_map_size(&response.headers) {
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

                    bytes_taken = usize::min(bytes_taken, hdr_partition_size);

                    if hdr_remaining_bytes <= hdr_partition_size {
                        if response.body.is_none() {
                            flags |= HeadersFlags::END_STREAM;
                        } else {
                            flags |= HeadersFlags::END_HEADERS;
                        }
                    }

                    frames.push(Frame::new(
                        stream.id,
                        flags.bits(),
                        FrameBody::Headers {
                            pad_length: None,
                            priority: None,
                            hdr_block_frag: header_bytes.slice(..bytes_taken),
                        },
                    ));
                } else {
                    let mut flags = ContinuationFlags::from_bits_retain(0);

                    if hdr_remaining_bytes <= hdr_partition_size {
                        flags |= ContinuationFlags::END_HEADERS;
                    }

                    frames.push(Frame::new(
                        stream_id,
                        flags.bits(),
                        FrameBody::Continuation {
                            hdr_block_frag: header_bytes.slice(..bytes_taken),
                        },
                    ));
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
                        flags |= DataFlags::END_STREAM;
                    }

                    frames.push(Frame::new(
                        stream_id,
                        flags.bits(),
                        FrameBody::Data {
                            pad_length: None,
                            data: body_bytes.slice(..bytes_taken),
                        },
                    ));

                    body_remaining_bytes -= bytes_taken;
                }
            }

            Ok(frames)
        }
    }

    /// Send a frame and update the frame's state machine accordingly.
    fn send_frame(&self, frame: Frame, tcp_writer: &mut BufWriter<TcpStream>) -> Result<(), ()> {
        let active_streams = self.active_streams.lock().unwrap();
        let mut stream = active_streams
            .get(&frame.header.stream_id)
            .ok_or(())?
            .lock()
            .unwrap();
        stream.send(&frame);
        let frame_bytes: Bytes = frame.try_into().map_err(|_| ())?;
        tcp_writer.write_all(frame_bytes.as_ref()).map_err(|_| ())?;
        tcp_writer.flush().map_err(|_| ())?;

        Ok(())
    }

    fn handle_request(&self, req: HTTPRequest, stream_id: u32, queue_tx: ResponseQueueTx) {
        let resp = (self.handler)(req);
        match self.make_frames(stream_id, resp) {
            Ok(frames) => {
                for frame in frames {
                    queue_tx.push(frame)
                }
            }
            Err(_) => (),
        }
    }

    fn run_rx(connection: Arc<Self>, queue_tx: ResponseQueueTx, mut tcp_reader: BufReader<TcpStream>) {
        println!("Rx thread started");
        loop {
            let frame  = match Frame::try_read_from_buf(&mut tcp_reader) {
                Ok(frame) => frame,
                Err(err) => {
                    match err {
                        frames::error::DeserializationError::BufReaderError => {},
                        _ => {
                            println!("Closing connection because of error {:#?}", err);
                            connection.close_with_error(ErrorCode::ProtocolError, 0, queue_tx.clone());
                        }
                    };
                    
                    break;
                }
            };

            let new_stream_required = frame.header.stream_id >= connection.stream_counter.load(Ordering::SeqCst); // TODO: Implement this
            if new_stream_required {
                let new_stream = connection.new_stream();
                let new_stream_id = new_stream.id;
                let mut active_streams = connection.active_streams.lock().unwrap();
                active_streams.insert(new_stream_id, Arc::new(Mutex::new(new_stream)));
            }

            // Section 8.1: Other frames (from any stream) MUST NOT occur between the HEADERS frame and any CONTINUATION frames that might follow
            {
                let active_streams = connection.active_streams.lock().unwrap();
                for stream_id in active_streams.keys() {
                    let stream = active_streams.get(stream_id).unwrap().lock().unwrap();
                    if *stream.get_req_assembler_state() == ReqAssemblerState::ReadingHeader {
                        match frame.payload {
                            FrameBody::Continuation { .. } => {},
                            _ => {
                                println!("stream {} received frame {:#?} while a Continuation is expected. ", stream_id, frame.header);
                                connection.close_with_error(ErrorCode::ProtocolError, frame.header.stream_id, queue_tx);
                                return; 
                            }
                        }
                    }
                }
            }
            

            let resp_frame: Result<Option<Frame>, ErrorCode> = match frame.payload.clone() {
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
                            connection.peer_settings.lock().unwrap().update_with_vec(&settings);

                            // Send ACK
                            let flag = SettingsFlags::ACK;
                            Ok(Some(Frame::new(
                                0,
                                flag.bits(),
                                FrameBody::Settings(vec![]),
                            )))
                        }
                    }
                },
                // See section 6.7
                FrameBody::Ping { data } => {
                    if frame.header.flags == PingFlags::ACK.bits() {
                        Ok(None)
                    } else {
                        Ok(Some(Frame::new(frame.header.stream_id, PingFlags::ACK.bits(), FrameBody::Ping { data })))
                    }
                },
                // See section 6.4
                FrameBody::RstStream { .. } => {
                    if frame.header.length != 4 {
                        Err(ErrorCode::FrameSizeError)
                    } else {
                        let active_streams = connection.active_streams.lock().unwrap();
                        let stream = active_streams.get(&frame.header.stream_id);
                        match stream {
                            Some(stream) => {
                                let stream = stream.lock().unwrap();
                                if stream.state == StreamState::Idle {
                                    Err(ErrorCode::ProtocolError)
                                } else {
                                    // Kill the stream
                                    Ok(None)
                                }
                            }, 
                            None => Err(ErrorCode::ProtocolError)
                        }
                    }
                }
                // Kill connection
                FrameBody::GoAway { .. } => {
                    break;
                }
                _ => Ok(None),
            };

            match resp_frame {
                Ok(Some(resp_frame)) => {
                    queue_tx.push(resp_frame);
                }
                Err(error_code) => {
                    connection.close_with_error(error_code, frame.header.stream_id, queue_tx);
                    break;
                }
                _ => {}
            }

            if frame.header.stream_id > 0 {
                let active_streams = connection.active_streams.lock().unwrap();
                let mut stream = active_streams
                    .get(&frame.header.stream_id)
                    .unwrap()
                    .lock()
                    .unwrap();
                let req = match stream.recv(frame.clone()) {
                    Ok(Some(req)) => {
                        let tx = queue_tx.clone();
                        let connection_cp = connection.clone();
                        thread::spawn(move || connection_cp.handle_request(req, frame.header.stream_id, tx));
                    }
                    Err(err) => {
                        // Close connection if connection error
                        connection.close_with_error(err, frame.header.stream_id, queue_tx);
                        break;
                    }
                    _ => {}
                };
                if stream.state == StreamState::Closed {
                    println!("Stream {} closed. ", stream.id);
                    connection.active_streams.lock().unwrap().remove(&stream.id);
                }
                req
            }
        }
    }

    fn run_tx(connection: Arc<Self>, queue_rx: ResponseQueueRx, mut tcp_writer: BufWriter<TcpStream>) {
        println!("Tx thread started");
        loop {
            let frame = match queue_rx.pop() {
                Ok(tagged_resp) => tagged_resp,
                Err(_) => {
                    break;
                }
            };

            let _ = connection.send_frame(frame, &mut tcp_writer);
            
        }

        // Close stream 0
        let _ = connection.send_frame(Frame::new(0, 0, FrameBody::RstStream { error_code: ErrorCode::NoError }), &mut tcp_writer);
    }

    fn close_with_error(
        &self,
        error_code: ErrorCode,
        last_stream_id: u32,
        queue_tx: ResponseQueueTx,
    ) {
        println!("Closing the connection with error {:#?}", error_code);
        let frame: Frame = Frame::new(
            0,
            0,
            FrameBody::GoAway {
                last_stream_id,
                error_code,
                additional_debug_data: Bytes::new(),
            },
        );
        queue_tx.push(frame);
    }

    pub fn run(self, tcp_reader: BufReader<TcpStream>, tcp_writer: BufWriter<TcpStream>) -> Option<()> {
        let (tx, rx) = mpsc::channel();
        let queue_tx = ResponseQueueTx::new(tx);
        let queue_rx = ResponseQueueRx::new(rx);

        let connection = Arc::new(self);
        let connection_cp = connection.clone();

        let rx_handle = thread::spawn(move || Self::run_rx(connection, queue_tx, tcp_reader));
        let tx_handle = thread::spawn(move || Self::run_tx(connection_cp, queue_rx, tcp_writer));
        rx_handle.join().unwrap();
        tx_handle.join().unwrap();

        println!("Connection Ended");
        None
    }
}

/// See RFC7540 section 6.5
#[repr(u16)]
#[derive(TryFromPrimitive, IntoPrimitive, Eq, PartialEq, Debug, Clone)]
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
            (
                SettingsIdentifier::EnablePush,
                0,
                1,
                ErrorCode::ProtocolError,
            ),
            (
                SettingsIdentifier::InitialWindowSize,
                0,
                2147483647,
                ErrorCode::FlowControlError,
            ),
            (
                SettingsIdentifier::MaxFrameSize,
                16384,
                16777215,
                ErrorCode::ProtocolError,
            ),
        ];
        match VALID_RANGES.iter().find(|entry| entry.0 == *self) {
            Some((_identifier, min, max, error_code)) => {
                if min <= &value && &value <= max {
                    Ok(value)
                } else {
                    Err(error_code.clone())
                }
            }
            None => Ok(value),
        }
    }
}
pub struct SettingsMap(HashMap<u16, u32>);
impl SettingsMap {
    /// Create a SettingsMap with default values
    pub fn default() -> Self {
        let mut settings_map = Self(HashMap::new());

        // Theses fields should ALWAYS exist within the lifespan of a SettingsMap
        settings_map.set(SettingsIdentifier::HeaderTableSize, 4096).unwrap();
        settings_map.set(SettingsIdentifier::EnablePush, 1).unwrap();
        settings_map.set(SettingsIdentifier::MaxConcurrentStreams, 128).unwrap();
        settings_map.set(SettingsIdentifier::InitialWindowSize, 65535).unwrap();
        settings_map.set(SettingsIdentifier::MaxFrameSize, 16384).unwrap();
        settings_map.set(SettingsIdentifier::MaxHeaderListSize, u32::MAX).unwrap();

        settings_map
    }

    pub fn set(&mut self, identifier: SettingsIdentifier, value: u32) -> Result<(), ErrorCode> {
        self.0
            .insert(identifier.clone() as u16, identifier.is_valid_value(value)?);
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

    pub fn update_with_vec(&mut self, other: &Vec<SettingParam>) {
        for setting in other {
            self.0.insert(setting.identifier.clone() as u16, setting.value);
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
            params.push(SettingParam {
                identifier: key.try_into().unwrap(),
                value,
            });
        }
        params
    }
}
