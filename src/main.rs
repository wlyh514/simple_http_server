use std::{
    fs,
    io::{prelude::*, BufReader},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex, mpsc::{self, Receiver, Sender}},
    thread::{self, JoinHandle},
    time::Duration,
    vec, collections::VecDeque,
};

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
    let response = format!("{status_line}\r\nContent-Length: {content_length}\r\n\r\n{contents}\r\n");

    resp_signal_tx.send(RespHandlerSignal::NewResp).unwrap();
    return response;
}

fn main() {
    let listener = TcpListener::bind("localhost:7878").unwrap();

    println!("Server started");

    for stream in listener.incoming() {
        let stream = stream.unwrap();

        handle_connection(stream);
    }
}