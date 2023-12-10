use rustls::{
    server::{Accepted, Acceptor},
    ServerConfig, ServerConnection, Stream,
};
use std::{
    collections::{HashMap, VecDeque},
    fs,
    io::{prelude::*, BufReader, Error},
    net::{TcpListener, TcpStream},
    sync::{
        mpsc::{self, Receiver, Sender},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
    vec, borrow::BorrowMut,
};

pub mod http;
pub mod http2;
pub mod tls;

use http::ResponseStatus;

use tls::choose_tls_config;

type ResponseQueue = VecDeque<JoinHandle<String>>;

enum RespHandlerSignal {
    NewResp,
    Finished,
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

fn handle_response(
    response_queue: Arc<Mutex<ResponseQueue>>,
    mut stream: TcpStream,
    resp_signal_rx: Receiver<RespHandlerSignal>,
) {
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
            }
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
    let mut splited_request_line = request_line.split_whitespace();
    let method = splited_request_line.next().unwrap();
    let uri = splited_request_line.next().unwrap();
    let _protocol = splited_request_line.next().unwrap();

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
    let response =
        format!("{status_line}\r\nContent-Length: {content_length}\r\n\r\n{contents}\r\n");

    resp_signal_tx.send(RespHandlerSignal::NewResp).unwrap();
    return response;
}

fn request_handler(req: http::HTTPRequest) -> http::HTTPResponse {
    let (status, file_name) = match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/") => (ResponseStatus::Ok, String::from("index.html")),
        (_, "/slow") => {
            // Simulate long processing time
            thread::sleep(Duration::from_secs(10));
            (ResponseStatus::Ok, String::from("index.html"))
        }
        ("GET", path) => (ResponseStatus::Ok, String::from(path)),
        _ => (ResponseStatus::NotFound, String::from("404.html")),
    };

    let mut response = http::HTTPResponse::default();
    response.status = status;

    let contents = match fs::read(format!("static/{file_name}")) {
        Ok(content) => content,
        Err(_) => {
            response.status = ResponseStatus::NotFound;
            vec![]
        }
    };
    let content_length = contents.len();

    response.set_multiple(HashMap::from([
        ("access-control-allow-origin", "*"),
        ("content-type", "text/html"),
        ("content-length", &format!("{content_length}")),
    ]));

    response.body = Some(contents.into());

    response
}

fn main() {
    let host: &str = "localhost:7878";
    let listener: TcpListener = TcpListener::bind(host).unwrap();
    let h2_server: http2::server::Server<fn(http::HTTPRequest) -> http::HTTPResponse> =
        http2::server::Server::new(request_handler);

    println!("Server started on {host}");

    // Start a TLS server that waits for incoming connections.
    for stream in listener.incoming() {
        let mut stream: TcpStream = stream.unwrap();
        let mut acceptor: Acceptor = Acceptor::default();

        // Read TLS packets until a full ClientHello is consumed. This signals the
        // server that it is ready to accept a connection.
        let accepted: Accepted = loop {
            acceptor.read_tls(&mut stream).unwrap();
            if let Some(accepted) = acceptor.accept().unwrap() {
                break accepted;
            }
        };

        // Choose a TLS configuration for the accepted connection which may be
        // modified by the ClientHello.
        let tls_config: Arc<ServerConfig> = choose_tls_config(accepted.client_hello());
        let mut connection: ServerConnection = accepted.into_connection(tls_config).unwrap();
        let _test: Result<(usize, usize), Error> = connection.complete_io(&mut stream);

        h2_server.handle_connection(connection, stream);
    }
}
