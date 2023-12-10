use std::{
    collections::VecDeque,
    io::{prelude::*, BufReader, Write},
    net::TcpStream,
    sync::{
        mpsc::{self, Receiver, Sender},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use bytes::{Bytes, BytesMut};

use crate::http::{HTTPRequest, HTTPResponse, HeaderVal, HeadersMap, ReqHandlerFn, ResponseStatus};
type ResponseQueue = VecDeque<JoinHandle<HTTPResponse>>;

enum RespHandlerSignal {
    NewResp,
    Finished,
}

pub struct Server<T: ReqHandlerFn + Copy + 'static> {
    handler: T,
}

impl<T: ReqHandlerFn + Copy + 'static> Server<T> {
    pub fn new(handler: T) -> Self {
        Self { handler }
    }

    pub fn handle_connection(&self, stream: TcpStream) {
        println!("new connection");
        let handler = self.handler;
        thread::spawn(move || Server::<T>::_handle_connection(stream, handler));
    }

    fn _handle_connection(stream: TcpStream, handler: T) {
        let response_queue: Arc<Mutex<ResponseQueue>> = Arc::new(Mutex::new(VecDeque::new()));

        let (resp_signal_tx, resp_signal_rx) = mpsc::channel::<RespHandlerSignal>();
        let response_queue_cp = response_queue.clone();
        let stream_cp = stream.try_clone().unwrap();
        thread::spawn(move || handle_response(response_queue_cp, stream_cp, resp_signal_rx));

        let buf_reader = BufReader::<TcpStream>::new(stream);

        let mut http_request_lines: Vec<String> = vec![];
        let _body = false;
        for line in buf_reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };
            println!("line read: {line}");

            if line.is_empty() {
                // Wrap up this request, send to request handler
                let resp_signal_tx_cp = resp_signal_tx.clone();

                let req = deserialize_req(&http_request_lines);
                let handler = thread::spawn(move || {
                    Server::<T>::handle_request(handler, req, resp_signal_tx_cp)
                });
                {
                    let mut response_queue = response_queue.lock().unwrap();
                    response_queue.push_back(handler);
                    println!("New handler pushed");
                }
                http_request_lines.clear();
            } else {
                http_request_lines.push(line);
            }
        }
        resp_signal_tx.send(RespHandlerSignal::Finished).unwrap();
    }

    fn handle_request<H: ReqHandlerFn>(
        handler: H,
        req: HTTPRequest,
        resp_signal_tx: Sender<RespHandlerSignal>,
    ) -> HTTPResponse {
        let mut resp = HTTPResponse::default();
        handler(req, &mut resp);
        resp_signal_tx.send(RespHandlerSignal::NewResp).unwrap();
        resp
    }
}

// Panics upon malformed requests, since this is not the focus of this project.
fn deserialize_req(lines: &Vec<String>) -> HTTPRequest {
    let request_line = lines[0].clone();
    let mut splited_request_line = request_line.split_whitespace();
    let method = splited_request_line.next().unwrap().to_string();
    let path = splited_request_line.next().unwrap().to_string();
    let protocol = splited_request_line.next().unwrap().to_string();

    let mut headers = HeadersMap::new();
    for line in &lines[1..] {
        let mut key = line.clone();
        let val = key.split_off(line.find(":").unwrap());
        let val = val[1..].trim();

        headers.insert(key, HeaderVal::Single(String::from(val)));
    }

    HTTPRequest {
        method,
        path,
        protocol,
        headers,
        body: None,
        trailers: None,
    }
}

fn serialize_res(res: &HTTPResponse) -> Bytes {
    let mut buf = BytesMut::new();

    let status_line = format!(
        "HTTP/1.1 {} {}",
        res.status.clone() as u32,
        status_to_string(&res.status)
    );
    let headers_string = headers_to_string(&res.headers);

    buf.extend(format!("{status_line}\r\n{headers_string}\r\n").bytes());
    if let Some(body_bytes) = &res.body {
        buf.extend_from_slice(&body_bytes);
    }

    buf.into()
}

fn headers_to_string(headers: &HeadersMap) -> String {
    let mut header_string = String::new();
    for (key, val) in headers {
        let val_string = match val {
            HeaderVal::Single(val) => val.clone(),
            HeaderVal::Multiple(vals) => vals.join(", "),
        };
        header_string += &format!("{key}: {val_string}\r\n");
    }
    header_string
}

fn status_to_string(status: &ResponseStatus) -> String {
    let variant = format!("{:#?}", status);
    let mut status_string = String::new();
    for chr in variant.chars() {
        if chr.is_uppercase() && status_string.len() > 0 {
            status_string += " ";
        }
        status_string += &chr.to_uppercase().to_string();
    }
    status_string
}

fn handle_response(
    response_queue: Arc<Mutex<ResponseQueue>>,
    mut stream: TcpStream,
    resp_signal_rx: Receiver<RespHandlerSignal>,
) {
    loop {
        match resp_signal_rx.recv() {
            Ok(RespHandlerSignal::NewResp) => {
                let mut response_queue = response_queue.lock().unwrap();
                while !response_queue.is_empty() {
                    let handler = response_queue.pop_front().unwrap();
                    thread::sleep(Duration::from_micros(1));
                    if handler.is_finished() {
                        let response = handler.join().unwrap();
                        stream.write_all(&serialize_res(&response)).unwrap();
                        println!("Response sent: {:#?}", response);
                    } else {
                        break;
                    }
                }
            }
            Ok(RespHandlerSignal::Finished) => {
                break;
            }
            Err(_) => break,
        }
    }
}
