use std::{
    collections::VecDeque,
    fs,
    io::{prelude::*, BufReader},
    net::{TcpListener, TcpStream},
    sync::{
        mpsc::{self, Receiver, Sender},
        Arc, Mutex,
    },
    thread::{self, JoinHandle},
    time::Duration,
    vec,
};

pub mod http;
pub mod http2;
pub mod tls;

use http::{HeaderVal, ResponseStatus};

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
    let (_status, file_name) = match (req.method.as_str(), req.path.as_str()) {
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

    response.set("access-control-allow-origin", "*");
    response.set("content-type", "text/html");
    response.set("content-length", &format!("{content_length}"));

    response.body = Some(contents.into());

    response
}

fn main() {
    let host: &str = "localhost:7878";
    let listener: TcpListener = TcpListener::bind(host).unwrap();
    let tls_config: Arc<rustls::ServerConfig> = tls::config_tls();

    let h2_server: http2::server::Server<fn(http::HTTPRequest) -> http::HTTPResponse> =
        http2::server::Server::new(request_handler);

    println!("Server started on {host}");

    for stream in listener.incoming() {
        let stream: TcpStream = stream.unwrap();

        h2_server.handle_connection(stream);
    }
}
