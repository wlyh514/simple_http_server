use bytes::{Bytes, BytesMut};

use crate::http::{hdr_map_size, HTTPRequest, HTTPResponse, HeadersMap, ReqHandlerFn};

use super::{
    error::ErrorCode,
    frames::{
        self,
        error::{BodyDeserializationError, DeserializationError, HeaderDeserializationError},
        ContinuationFlags, Frame, FrameBody, HeadersFlags, PingFlags, SettingsFlags,
    },
    settings::{SettingParam, SettingsIdentifier, SettingsMap},
    stream::{compress_header, ReqAssemblerState, Stream, StreamState},
};
use ::std::{
    collections::HashMap,
    io::{BufReader, Write},
    net::TcpStream,
    sync::{
        atomic::{AtomicU32, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
};
use std::io::BufWriter;

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
    available_size: Arc<AtomicU32>,
    streams: Arc<Mutex<HashMap<u32, Arc<Mutex<Stream>>>>>,
    handler: T,
    id: usize,
}

impl<T: ReqHandlerFn + Copy + 'static> Connection<T> {
    pub fn new(
        handler: T,
        server_settings: SettingsMap,
        peer_settings_params: Vec<SettingParam>,
        id: usize,
    ) -> Connection<T> {
        let mut peer_settings = SettingsMap::default();
        peer_settings
            .update_with_vec(&peer_settings_params)
            .unwrap();
        Connection {
            server_settings: Arc::new(Mutex::new(server_settings)),
            peer_settings: Arc::new(Mutex::new(peer_settings)),
            max_stream_id: Arc::new(AtomicU32::new(0)),
            concurrent_stream_count: Arc::new(AtomicU32::new(1)),
            available_size: Arc::new(AtomicU32::new(65535)),
            streams: Arc::new(Mutex::new(HashMap::new())),
            handler,
            id,
        }
    }

    fn get_window_size(&self) -> u32 {
        self.available_size.load(Ordering::SeqCst)
    }

    fn set_window_size(&self, size: u32) {
        self.available_size.store(size, Ordering::SeqCst)
    }

    fn new_stream(&self, new_stream_id: u32) {
        assert!(new_stream_id > self.max_stream_id.load(Ordering::SeqCst));
        let new_stream = Stream::new(new_stream_id, self.peer_settings.clone());

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
    fn make_header_frames(
        &self,
        stream_id: u32,
        response: &mut HTTPResponse,
    ) -> Result<Vec<Frame>, ()> {
        let streams = self.streams.lock().unwrap();
        let stream = streams
            .get(&stream_id)
            .map_or(Err(()), |val| Ok(val))?
            .lock()
            .unwrap();

        // Write response pseudoheaders
        let status_code = response.status.clone() as u32;
        response.set(":status", &status_code.to_string());

        let header_bytes = compress_header(&response.headers);

        // Load relavent settings
        let hdr_partition_size: usize;
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
        let mut resp = HTTPResponse::default();
        (self.handler)(req, &mut resp);

        // Convert field name to lowercase
        let mut header_lower = HeadersMap::new();
        for (key, val) in resp.headers {
            header_lower.insert(key.to_lowercase(), val);
        }
        resp.headers = header_lower;

        match self.make_header_frames(stream_id, &mut resp) {
            Ok(frames) => {
                for frame in frames {
                    queue_tx.push(frame)
                }
                if let Some(body) = resp.body {
                    if let Some(stream) = self.streams.lock().unwrap().get(&stream_id) {
                        let mut stream = stream.lock().unwrap();
                        stream.push_data(body);
                        if let Some(data_frames) = stream.get_ready_data_frames() {
                            for frame in data_frames {
                                queue_tx.push(frame);
                            }
                        }
                    }
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

    fn run_rx(
        connection: Arc<Self>,
        queue_tx: ResponseQueueTx,
        mut tcp_reader: BufReader<TcpStream>,
    ) {
        println!("Rx thread started");
        loop {
            let mut unknown_frame = false;
            let frame = match Frame::try_read_from_buf(&mut tcp_reader) {
                Ok(frame) => frame,
                Err(DeserializationError::Header(
                    HeaderDeserializationError::UnknownFrameType(_),
                )) => {
                    // Section 5.5: No unknown/extension frame can be received within header block.
                    unknown_frame = true;

                    let zeroed_data: Bytes = BytesMut::zeroed(8).into();
                    Frame::new(0, 0, FrameBody::Ping { data: zeroed_data })
                }
                Err(DeserializationError::Body(
                    BodyDeserializationError::UnknownSettingsIdentifier(_),
                ))
                | Err(DeserializationError::Body(BodyDeserializationError::UnknownErrorCode(_))) => {
                    let zeroed_data: Bytes = BytesMut::zeroed(8).into();
                    Frame::new(0, 0, FrameBody::Ping { data: zeroed_data })
                }
                Err(err) => {
                    match err {
                        frames::error::DeserializationError::BufReaderError => break,
                        _ => {
                            println!("Closing connection because error occured during frame deserialization: {:#?}", err);
                            connection.close_with_error(
                                ErrorCode::ProtocolError,
                                0,
                                queue_tx.clone(),
                            );
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
                    connection.close_with_error(
                        ErrorCode::ProtocolError,
                        frame.header.stream_id,
                        queue_tx.clone(),
                    );
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
                            connection.close_with_error(
                                ErrorCode::ProtocolError,
                                frame.header.stream_id,
                                queue_tx.clone(),
                            );
                            return;
                        }
                        match frame.payload {
                            FrameBody::Continuation { .. } => {}
                            _ => {
                                println!("stream {} received frame {:#?} while a Continuation is expected. ", stream_id, frame.header);
                                connection.close_with_error(
                                    ErrorCode::ProtocolError,
                                    frame.header.stream_id,
                                    queue_tx.clone(),
                                );
                                return;
                            }
                        }
                    }
                }
            }

            {
                let max_frame_size = connection
                    .peer_settings
                    .lock()
                    .unwrap()
                    .get(SettingsIdentifier::MaxFrameSize)
                    .unwrap() as usize;
                if frame.header.length > max_frame_size {
                    println!("Closing connection {} because it receives a frame of length {}, while the max frame size is {}. ", connection.id, frame.header.length, max_frame_size);
                    connection.close_with_error(
                        ErrorCode::FrameSizeError,
                        frame.header.stream_id,
                        queue_tx.clone(),
                    );
                }
            }

            // Flow control
            if let FrameBody::Settings(settings) = &frame.payload {
                for param in settings {
                    if param.identifier == SettingsIdentifier::InitialWindowSize {
                        let window_size_delta = param.value as i64
                            - connection
                                .peer_settings
                                .lock()
                                .unwrap()
                                .get(SettingsIdentifier::InitialWindowSize)
                                .unwrap() as i64;
                        let streams = connection.streams.lock().unwrap();
                        for (_, stream) in streams.iter() {
                            let mut stream = stream.lock().unwrap();
                            if stream.state.is_active() {
                                match stream.window_update(window_size_delta) {
                                    Ok(_) => {
                                        if let Some(data_frames) = stream.get_ready_data_frames() {
                                            for frame in data_frames {
                                                queue_tx.push(frame);
                                            }
                                        }
                                    }
                                    Err(_) => {
                                        println!(
                                            "Closing connection {} because window size overflow.",
                                            connection.id
                                        );
                                        connection.close_with_error(
                                            ErrorCode::FlowControlError,
                                            frame.header.stream_id,
                                            queue_tx.clone(),
                                        );
                                        break;
                                    }
                                }
                            }
                        }
                    }
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

            // Flow control
            if let FrameBody::WindowUpdate {
                window_size_increment,
            } = &frame.payload
            {
                match frame.header.stream_id {
                    0 => {
                        // A connection-wide window update
                        let new_window_size = connection.get_window_size() + window_size_increment;
                        if new_window_size > 2147483647 {
                            println!(
                                "Closing connection {} because window size overflow.",
                                connection.id
                            );
                            connection.close_with_error(
                                ErrorCode::FlowControlError,
                                frame.header.stream_id,
                                queue_tx.clone(),
                            );
                            break;
                        }
                        connection.set_window_size(new_window_size);
                        // Notify tx
                        // tx_handle.thread().unpark();
                    }
                    stream_id => match connection.streams.lock().unwrap().get(&stream_id) {
                        Some(stream) => {
                            let mut stream = stream.lock().unwrap();
                            match stream.window_update(*window_size_increment as i64) {
                                Ok(_) => {
                                    if let Some(data_frames) = stream.get_ready_data_frames() {
                                        for frame in data_frames {
                                            queue_tx.push(frame);
                                        }
                                    }
                                }
                                Err(_) => {
                                    println!(
                                        "Closing stream {} because window size overflow.",
                                        stream.id
                                    );
                                    queue_tx.push(Frame::new(
                                        stream.id,
                                        0,
                                        FrameBody::RstStream {
                                            error_code: ErrorCode::FlowControlError,
                                        },
                                    ))
                                }
                            }
                        }
                        None => {
                            println!("Closing connection {} because the stream it refers to does not exist.", connection.id);
                            connection.close_with_error(
                                ErrorCode::FlowControlError,
                                frame.header.stream_id,
                                queue_tx.clone(),
                            );
                            break;
                        }
                    },
                }
            }

            if frame.header.stream_id > 0 {
                let streams = connection.streams.lock().unwrap();
                let stream_id = frame.header.stream_id;
                let mut stream = streams.get(&stream_id).unwrap().lock().unwrap();

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
                            e => {
                                println!("Error occurred while updating the stream state machine.");
                                connection.close_with_error(e, stream_id, queue_tx);
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
                        connection
                            .concurrent_stream_count
                            .fetch_sub(1, Ordering::SeqCst);
                    } else if !stream_active_before && stream_active_after {
                        connection
                            .concurrent_stream_count
                            .fetch_add(1, Ordering::SeqCst);
                    }
                    let max_concurrent_streams = connection
                        .server_settings
                        .lock()
                        .unwrap()
                        .get(SettingsIdentifier::MaxConcurrentStreams)
                        .unwrap();
                    if connection.concurrent_stream_count.load(Ordering::SeqCst)
                        > max_concurrent_streams
                    {
                        println!(
                            "Concurrent streams exceeds limit, closing connection {}.",
                            connection.id
                        );
                        connection.close_with_error(
                            ErrorCode::ProtocolError,
                            stream_id,
                            queue_tx.clone(),
                        );
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

    fn run_tx(
        connection: Arc<Self>,
        queue_rx: ResponseQueueRx,
        mut tcp_writer: BufWriter<TcpStream>,
    ) {
        println!("Tx thread started");
        loop {
            let frame = match queue_rx.pop() {
                Ok(tagged_resp) => tagged_resp,
                Err(_) => {
                    break;
                }
            };

            // Connection level flow control
            // if let FrameBody::Data{ .. } = frame.payload {
            //     let available_size = connection.get_window_size();
            //     let payload_size = frame.payload.size(); // Make it pub!
            //     while payload_size > available_size as usize {
            //         thread::park();
            //         let available_size = connection.get_window_size();
            //     }
            // }

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
        println!(
            "Closing connection {} with error {:#?}",
            self.id, error_code
        );
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

    pub fn run(
        self,
        tcp_reader: BufReader<TcpStream>,
        tcp_writer: BufWriter<TcpStream>,
    ) -> Option<()> {
        let (tx, rx) = mpsc::channel();
        let queue_tx = ResponseQueueTx::new(tx);
        let queue_rx = ResponseQueueRx::new(rx);
        let id = self.id;

        let connection = Arc::new(self);
        let connection_cp = connection.clone();

        let rx_handle = thread::spawn(move || Self::run_rx(connection, queue_tx, tcp_reader));
        let tx_handle = thread::spawn(move || Self::run_tx(connection_cp, queue_rx, tcp_writer));
        rx_handle.join().unwrap();
        tx_handle.join().unwrap();

        println!("Connection {id} Ended");
        None
    }
}
