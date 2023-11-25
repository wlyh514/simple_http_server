use std::{
    fs,
    io::{prelude::*, BufReader},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex, mpsc::{self, Receiver, Sender}},
    thread::{self, JoinHandle},
    time::Duration,
    vec, collections::VecDeque,
};
pub mod http;
pub mod http2;

use bytes::BytesMut;
use http::{ResponseStatus, HeaderVal};
use http2::{connection::SettingsMap, frames::FrameBody};


type ResponseQueue = VecDeque<JoinHandle<String>>;

enum RespHandlerSignal {
    NewResp, 
    Finished,
}

/// Section 3.5
fn handle_h2_connection(mut stream: TcpStream) {
    // recv preface
    let mut preface_starter = BytesMut::with_capacity(24);
    stream.read_exact(&mut preface_starter);

    let client_settings: Option<SettingsMap> = if let Ok(preface_starter) = String::from_utf8(preface_starter.to_vec()) {
        if preface_starter == "PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n" {
            let reader = BufReader::new(stream);
            if let Ok(preface_frame) = http2::frames::Frame::try_read_from_buf(reader) {
                if let FrameBody::Settings(settings_params) = preface_frame.payload {
                    let mut settings = SettingsMap::new();
                    for param in settings_params {
                        settings.set(param.identifier, param.value);
                    }
                    Some(settings)
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    let frame = http2::frames::Frame::new()


}

fn handle_connection(stream: TcpStream) {
    let response_queue: Arc<Mutex<ResponseQueue>> = Arc::new(Mutex::new(VecDeque::new()));
    
    let (resp_signal_tx, resp_signal_rx) = mpsc::channel::<RespHandlerSignal>();
    let response_queue_cp = response_queue.clone();
    let stream_cp = stream.try_clone().unwrap();
    thread::spawn(move || handle_response(response_queue_cp, stream_cp, resp_signal_rx));

    let buf_reader = BufReader::<TcpStream>::new(stream);

    let mut http_request: Vec<String> = vec![];
    let _body = false;
    for line in buf_reader.lines() {
        let line = line.unwrap();

        if line.is_empty() {
            // TODO: handle the case of request with body
            
            // Wrap up this request, send to request handler
            let http_request_cp = http_request.clone();
            let resp_signal_tx_cp = resp_signal_tx.clone();
            let handler = thread::spawn(move || handle_request(http_request_cp, resp_signal_tx_cp));
            {
                let mut response_queue = response_queue.lock().unwrap();
                response_queue.push_back(handler);
                println!("New handler pushed");
            }
            http_request.clear();
        } else {
            http_request.push(line);
        }
    }
    resp_signal_tx.send(RespHandlerSignal::Finished).unwrap();
}

fn handle_response(response_queue: Arc<Mutex<ResponseQueue>>, mut stream: TcpStream, resp_signal_rx: Receiver<RespHandlerSignal>) {
    loop {
        match resp_signal_rx.recv().unwrap() {
            RespHandlerSignal::NewResp => {
                let mut response_queue = response_queue.lock().unwrap();
                while !response_queue.is_empty() {
                    let handler = response_queue.pop_front().unwrap();
                    thread::sleep(Duration::from_micros(1));
                    if handler.is_finished() {
                        let response = handler.join().unwrap();
                        stream.write_all(response.as_bytes()).unwrap();
                        println!("Response sent: {response}");
                    } else {
                        break;
                    }
                }
            }, 
            RespHandlerSignal::Finished => {
                break;
            }
        }
    }
}

/// For old HTTP/1.1 code
fn handle_request(http_request: Vec<String>, resp_signal_tx: Sender<RespHandlerSignal>) -> String {
    println!("Request: {:#?}", &http_request);
    let request_line = http_request[0].clone();
    let (status_line, file_name) = match (method, uri) {
        ("GET", "/") => ("HTTP/1.1 200 OK", "index.html"),
        (_, "/slow") => {
            // Simulate long processing time
            thread::sleep(Duration::from_secs(10));
            ("HTTP/1.1 200 OK", "index.html")
        }
        _ => ("HTTP/1.1 404 NOT FOUND", "404.html"),
    };

    let contents = fs::read_to_string(format!("static/{file_name}")).unwrap();
    let content_length = contents.len();
    let response = format!("{status_line}\r\nContent-Length: {content_length}\r\n\r\n{contents}\r\n");

    resp_signal_tx.send(RespHandlerSignal::NewResp).unwrap();
    return response;
}

fn request_handler(req: http::HTTPRequest) -> http::HTTPResponse {
    let (status, file_name) = match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/") => (ResponseStatus::Ok, "index.html"),
        (_, "/slow") => {
            // Simulate long processing time
            thread::sleep(Duration::from_secs(10));
            (ResponseStatus::Ok, "index.html")
        }
        _ => (ResponseStatus::NotFound, "404.html"),
    };

    let contents = fs::read(format!("static/{file_name}")).unwrap();
    let content_length = contents.len();
    let mut response = http::HTTPResponse::default();

    response.headers.insert("content-length".into(), HeaderVal::Single("{content_length}".into()));
    response.body = Some(contents.into());

    response
}

fn main() {
    let listener = TcpListener::bind("localhost:7878").unwrap();

    let h2_server = http2::server::Server::new(request_handler);

    println!("Server started");

    for stream in listener.incoming() {
        let stream = stream.unwrap();

        h2_server.handle_connection(stream);
    }
}