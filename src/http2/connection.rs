use ::num_enum::{IntoPrimitive, TryFromPrimitive};
use bytes::{Bytes, BytesMut};

use crate::{
    http::{hdr_map_size, HTTPRequest, HTTPResponse, HeaderVal, ReqHandlerFn},
    http2::{stream::{StreamState, ReqAssemblerState}, frames::{self, PingFlags, error::{DeserializationError, BodyDeserializationError, HeaderDeserializationError}}},
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
    server_settings: Arc<Mutex<SettingsMap>>, 
    peer_settings: Arc<Mutex<SettingsMap>>,
    max_stream_id: Arc<AtomicU32>,
    concurrent_stream_count: Arc<AtomicU32>,
    streams: Arc<Mutex<HashMap<u32, Arc<Mutex<Stream>>>>>,
    handler: T,
    id: usize,
}

impl<T: ReqHandlerFn + Copy + 'static> Connection<T> {
    pub fn new(
        handler: T,
        server_settings: SettingsMap,
        peer_settings_params: Vec<SettingParam>,
        id: usize
    ) -> Connection<T> {
        let mut peer_settings = SettingsMap::default();
        peer_settings.update_with_vec(&peer_settings_params).unwrap();
        Connection {
            server_settings: Arc::new(Mutex::new(server_settings)),
            peer_settings: Arc::new(Mutex::new(peer_settings)),
            max_stream_id: Arc::new(AtomicU32::new(0)),
            concurrent_stream_count: Arc::new(AtomicU32::new(1)),
            streams: Arc::new(Mutex::new(HashMap::new())),
            handler,
            id
        }
    }

    fn new_stream(&self, new_stream_id: u32) {
        assert!(new_stream_id > self.max_stream_id.load(Ordering::SeqCst));
        let new_stream = Stream::new(new_stream_id);

        {
            // Close all idle streams with id smaller than the new stream
            let mut streams = self.streams.lock().unwrap();
            let mut streams_to_close: Vec<u32> = vec![];
            for (id, stream) in streams.iter() {
                let stream = stream.lock().unwrap();
                if stream.state == StreamState::Idle {
                    streams_to_close.push(*id);
                }
            }
            for id in streams_to_close {
                streams.remove(&id);
            }
            streams.insert(new_stream_id, Arc::new(Mutex::new(new_stream)));
        }

        self.max_stream_id.store(new_stream_id, Ordering::SeqCst);
    }

    /// Convert a http response into frames to be sent.
    fn make_frames(&self, stream_id: u32, mut response: HTTPResponse) -> Result<Vec<Frame>, ()> {
        let streams = self.streams.lock().unwrap();
        let stream = streams
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
        dbg!(self.id, "send", &frame);

        // If the stream exists, update its state machine.
        // Otherwise we skip this step, which would happen if:
        // a) We are sending a GOAWAY frame
        // b) We are sending a PRIORITY frame for a stream in CLOSED state.
        let streams = self.streams.lock().unwrap();
        if let Some(stream) = streams.get(&frame.header.stream_id) {
            stream.lock().unwrap().send(&frame);
        }
        
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

    fn handle_frame(&self, frame: &Frame) -> Result<Option<Frame>, ErrorCode> {
        match &frame.payload {
            // Section 6.1
            FrameBody::Data { pad_length, .. } => {
                if frame.header.stream_id == 0x0 {
                    Err(ErrorCode::ProtocolError)
                } else if frame.header.length <= pad_length.unwrap_or(0) {
                    Err(ErrorCode::ProtocolError)
                } else {
                    let streams = self.streams.lock().unwrap();
                    match streams.get(&frame.header.stream_id) {
                        Some(stream) => {
                            let stream = stream.lock().unwrap();
                            if stream.state != StreamState::Open && stream.state != StreamState::HalfClosedRemote {
                                Err(ErrorCode::StreamClosed)
                            } else {
                                Ok(None)
                            }
                        },
                        None => {
                            Err(ErrorCode::StreamClosed)
                        }
                    }
                }
            },
            // Section 6.2
            FrameBody::Headers { pad_length, priority, .. } => {
                if frame.header.stream_id == 0x0 {
                    Err(ErrorCode::ProtocolError)
                } else if frame.header.length <= pad_length.unwrap_or(0) {
                    Err(ErrorCode::ProtocolError)
                } else if let Some(priority) = priority {
                    if priority.stream_dep == frame.header.stream_id {
                        Err(ErrorCode::ProtocolError)
                    } else {
                        Ok(None)
                    }
                } else {
                    Ok(None)
                }
            }, 
            // Section 6.3
            FrameBody::Priority { stream_dep, .. } => {
                if frame.header.stream_id == 0x0 || frame.header.stream_id == *stream_dep {
                    Err(ErrorCode::ProtocolError)
                } else if frame.header.length != 5 {
                    Err(ErrorCode::FrameSizeError)
                } else {
                    Ok(None)
                }
            },
            // Section 6.4
            FrameBody::RstStream { .. } => {
                if frame.header.length != 4 {
                    Err(ErrorCode::FrameSizeError)
                } else {
                    let streams = self.streams.lock().unwrap();
                    let stream = streams.get(&frame.header.stream_id);
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
            },
            // Section 6.5
            FrameBody::Settings(settings) => {
                let flags = SettingsFlags::from_bits_retain(frame.header.flags);

                // If settings frame is sent over stream other than 0x0, connection error
                if frame.header.stream_id != 0x0 {
                    Err(ErrorCode::ProtocolError)
                } else if frame.header.length % 6 != 0 {
                    Err(ErrorCode::FrameSizeError)
                } else {
                    if flags.contains(SettingsFlags::ACK) {
                        if frame.header.length != 0 {
                            Err(ErrorCode::FrameSizeError)
                        } else {
                            Ok(None)
                        }
                    } else {
                        // Update self.settings, return an error if there's any inappropriate value.
                        self.peer_settings.lock().unwrap().update_with_vec(&settings)?; 
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
            // Section 6.6
            FrameBody::PushPromise { .. } => {
                // Receipt of PUSH_PROMISE frames is protocol error as we do not enable push.
                Err(ErrorCode::ProtocolError)
            },
            // Section 6.7
            FrameBody::Ping { data } => {
                if frame.header.stream_id != 0x0 {
                    Err(ErrorCode::ProtocolError)
                } else if frame.header.length != 8 {
                    Err(ErrorCode::FrameSizeError)
                } else if frame.header.flags == PingFlags::ACK.bits() {
                    Ok(None)
                } else {
                    Ok(Some(Frame::new(frame.header.stream_id, PingFlags::ACK.bits(), FrameBody::Ping { data: data.clone() })))
                }
            },
            // Section 6.8
            FrameBody::GoAway { .. } /* { last_stream_id, error_code, .. } */ => {
                Err(ErrorCode::NoError)
            },
            // Section 6.9
            FrameBody::WindowUpdate { window_size_increment } => {
                if frame.header.length != 4 {
                    Err(ErrorCode::FrameSizeError)
                } else if *window_size_increment == 0 {
                    Err(ErrorCode::FrameSizeError)
                } else {
                    Ok(None)
                }
            },
            // Section 6.10
            FrameBody::Continuation { .. } => {
                if frame.header.stream_id == 0x0 {
                    Err(ErrorCode::ProtocolError)
                } else {
                    Ok(None)
                }
            }
        }
    }

    fn run_rx(connection: Arc<Self>, queue_tx: ResponseQueueTx, mut tcp_reader: BufReader<TcpStream>) {
        println!("Rx thread started");
        loop {
            let mut unknown_frame = false;
            let frame = match Frame::try_read_from_buf(&mut tcp_reader) {
                Ok(frame) => frame,
                Err(DeserializationError::Header(HeaderDeserializationError::UnknownFrameType(_)))  => {
                    // Section 5.5: No unknown/extension frame can be received within header block. 
                    unknown_frame = true;

                    let zeroed_data: Bytes = BytesMut::zeroed(8).into();
                    Frame::new(0, 0, FrameBody::Ping { data: zeroed_data })
                },
                Err(DeserializationError::Body(BodyDeserializationError::UnknownSettingsIdentifier(_))) |
                Err(DeserializationError::Body(BodyDeserializationError::UnknownErrorCode(_))) => {
                    let zeroed_data: Bytes = BytesMut::zeroed(8).into();
                    Frame::new(0, 0, FrameBody::Ping { data: zeroed_data })
                }
                Err(err) => {
                    match err {
                        frames::error::DeserializationError::BufReaderError => break,
                        _ => {
                            println!("Closing connection because error occured during frame deserialization: {:#?}", err);
                            connection.close_with_error(ErrorCode::ProtocolError, 0, queue_tx.clone());
                        }
                    };
                    
                    break;
                }
            };
            dbg!(connection.id, "recv", &frame);

            if frame.header.stream_id > connection.max_stream_id.load(Ordering::SeqCst) {
                if frame.header.stream_id % 2 == 1 {
                    connection.new_stream(frame.header.stream_id);
                } else {
                    println!("Client initiates an even numbered stream id, closing connection. ");
                    connection.close_with_error(ErrorCode::ProtocolError, frame.header.stream_id, queue_tx.clone());
                    break;
                }
            }

            // Section 8.1: Other frames (from any stream) MUST NOT occur between the HEADERS frame and any CONTINUATION frames that might follow
            {
                let streams = connection.streams.lock().unwrap();
                for stream_id in streams.keys() {
                    let stream = streams.get(stream_id).unwrap().lock().unwrap();
                    dbg!(stream_id, stream.get_req_assembler_state());
                    if *stream.get_req_assembler_state() == ReqAssemblerState::ReadingHeader {
                        if unknown_frame {
                            println!("stream {} received unknown/extension frame while a Continuation is expected. ", stream_id);
                            connection.close_with_error(ErrorCode::ProtocolError, frame.header.stream_id, queue_tx.clone());
                            return; 
                        }
                        match frame.payload {
                            FrameBody::Continuation { .. } => {},
                            _ => {
                                println!("stream {} received frame {:#?} while a Continuation is expected. ", stream_id, frame.header);
                                connection.close_with_error(ErrorCode::ProtocolError, frame.header.stream_id, queue_tx.clone());
                                return; 
                            }
                        }
                    }
                }
            }
            
            {
                let max_frame_size = connection.peer_settings.lock().unwrap().get(SettingsIdentifier::MaxFrameSize).unwrap() as usize;
                if frame.header.length > max_frame_size {
                    println!("Closing connection {} because it receives a frame of length {}, while the max frame size is {}. ", connection.id, frame.header.length, max_frame_size);
                    connection.close_with_error(ErrorCode::FrameSizeError, frame.header.stream_id, queue_tx.clone());
                }
            }

            match connection.handle_frame(&frame) {
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
                let streams = connection.streams.lock().unwrap();
                let stream_id = frame.header.stream_id;
                let mut stream = streams
                    .get(&stream_id)
                    .unwrap()
                    .lock()
                    .unwrap();

                let stream_active_before = stream.state.is_active(); 
                match stream.recv(frame) {
                    Ok(Some(req)) => {
                        let tx = queue_tx.clone();
                        let connection_cp = connection.clone();
                        thread::spawn(move || connection_cp.handle_request(req, stream_id, tx));
                    }
                    Err(err) => {
                        // Close connection if connection error
                        match err {
                            // ErrorCode::StreamClosed => {
                            //     queue_tx.push(Frame::new(stream_id, 0, FrameBody::RstStream { error_code: ErrorCode::StreamClosed }));
                            //     println!("STREAM_CLOSED queued");
                            // },
                            _ => {
                                println!("Error occurred while updating the stream state machine.");
                                connection.close_with_error(err, stream_id, queue_tx);
                                break;
                            }
                        }
                    }
                    _ => {}
                };
                let stream_active_after = stream.state.is_active();

                // Section 5.1.2
                {
                    if stream_active_before && !stream_active_after {
                        connection.concurrent_stream_count.fetch_sub(1, Ordering::SeqCst);
                    } else if !stream_active_before && stream_active_after {
                        connection.concurrent_stream_count.fetch_add(1, Ordering::SeqCst);
                    }
                    let max_concurrent_streams = connection.server_settings.lock().unwrap().get(SettingsIdentifier::MaxConcurrentStreams).unwrap();
                    if connection.concurrent_stream_count.load(Ordering::SeqCst) > max_concurrent_streams {
                        println!("Concurrent streams exceeds limit, closing connection {}.", connection.id); 
                        connection.close_with_error(ErrorCode::ProtocolError, stream_id, queue_tx.clone());
                        break;
                    }
                }
                
                
                if stream.state == StreamState::Closed {
                    println!("Stream {} closed. ", stream.id);
                    // connection.streams.lock().unwrap().remove(&stream.id);
                }
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
        // let _ = connection.send_frame(Frame::new(0, 0, FrameBody::RstStream { error_code: ErrorCode::NoError }), &mut tcp_writer);
    }

    fn close_with_error(
        &self,
        error_code: ErrorCode,
        last_stream_id: u32,
        queue_tx: ResponseQueueTx,
    ) {
        println!("Closing connection {} with error {:#?}", self.id, error_code);
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
#[derive(Clone)]
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

    pub fn update_with_vec(&mut self, other: &Vec<SettingParam>) -> Result<(), ErrorCode> {
        for setting in other {
            self.set(setting.identifier.clone(), setting.value)?;
        }
        Ok(())
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
