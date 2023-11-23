use ::num_enum::{TryFromPrimitive, IntoPrimitive};

use crate::http::{ReqHandlerFn, HTTPRequest, HTTPResponse};

use super::{frames::Frame, stream::Stream};
use ::std::{collections::HashMap, net::TcpStream, thread, sync::{Arc, Mutex, mpsc}};

pub type TaggedResponse = (u32, HTTPResponse);

enum ResponseQueueError {
    Terminated
}
struct ResponseQueueRx(mpsc::Receiver<TaggedResponse>);
#[derive(Clone)]
struct ResponseQueueTx(mpsc::Sender<TaggedResponse>);

impl ResponseQueueRx {
    pub fn new(rx: mpsc::Receiver<TaggedResponse>) -> Self {
        Self(rx)
    }

    pub fn pop(&self) -> Result<TaggedResponse, ResponseQueueError> {
        self.0.recv().map_err(|_| ResponseQueueError::Terminated)
    }
}

impl ResponseQueueTx {
    pub fn new(tx: mpsc::Sender<TaggedResponse>) -> Self {
        Self(tx)
    }

    pub fn push(&self, resp: TaggedResponse) {
        match self.0.send(resp) { _ => () }
    }
}

pub struct Connection<T: ReqHandlerFn + Sync> {
    tcp_stream: TcpStream,
    settings: SettingsMap,
    stream_counter: u32,
    active_streams: Arc<Mutex<HashMap<u32, Stream>>>,
    handler: T
}

impl<T: ReqHandlerFn + Sync> Connection<T> {
    pub fn new(tcp_stream: TcpStream, handler: T) -> Connection<T> {
        Connection {
            tcp_stream,
            settings: SettingsMap::new(),
            stream_counter: 1,
            active_streams: Arc::new(Mutex::new(HashMap::new())),
            handler,
        }
    }

    fn new_stream(&mut self) -> Stream {
        // TODO: Implement this
    }

    fn read_frame(&self) -> Frame {
        // TODO: Implement this
    }

    fn make_frames(stream: Stream, response: HTTPResponse) -> Vec<Frame> {
        // TODO: Implement this
        vec![]
    }

    fn send_frame(&self, frame: Frame) -> Result<(), ()> {
        // TODO: Implement this
        Ok(())
    }

    fn handle_request(&self, req: HTTPRequest, stream_id: u32, queue_tx: ResponseQueueTx) {
        let resp = (self.handler)(req);
        queue_tx.push((stream_id, resp));
    }

    fn run_rx(&self, queue_tx: ResponseQueueTx) {
        loop {
            let frame = self.read_frame();
            let new_stream_required = false;
            let stream: Stream = match new_stream_required {
                true => {
                    let new_stream = self.new_stream();
                    self.active_streams.push(new_stream);
                    new_stream
                }, 
                false => {
                    *self.active_streams.get(114514).unwrap()
                },
            };
            let result = stream.recv(frame);

            match result {
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
            let (stream_id, response) = match queue_rx.pop() {
                Ok(tagged_resp) => { tagged_resp },
                Err(_) => { break; }
            };
            if let Some(stream) = self.active_streams.lock().unwrap().get(&stream_id) {
                let frames = Self::make_frames(*stream, response);
                for frame in frames {
                    self.send_frame(frame);
                }
            }
        }
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
    MaxCurrentStream = 0x3,
    InitialWindowSize = 0x4,
    MaxFrameSize = 0x5,
    MaxHeaderListSize = 0x6,
}
type SettingsFrame = [u8; 6];
pub struct SettingsMap (HashMap<SettingsIdentifier, u32>);
