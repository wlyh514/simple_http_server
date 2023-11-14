use std::{fs, net::{TcpListener, TcpStream}, io::{BufReader, prelude::*}};

fn handle_connection(mut stream: TcpStream) {
    let buf_reader = BufReader::new(&mut stream);

    // println!("Request: {:#?}", http_request);
    let request_line = buf_reader.lines().next().unwrap().unwrap();
    let mut splited_request_line = request_line.split_whitespace();

    let method = splited_request_line.next().unwrap();
    let uri = splited_request_line.next().unwrap(); 
    let protocol = splited_request_line.next().unwrap();

    let (status_line, file_name) = match (method, uri) {
        ("GET", "/") => ("HTTP/1.1 200 OK", "index.html"), 
        _ => ("HTTP/1.1 404 NOT FOUND", "404.html"),
    };

    let contents = fs::read_to_string(format!("static/{file_name}")).unwrap();
    let content_length = contents.len();

    let response = format!("{status_line}\r\nContent-Length: {content_length}\r\n\r\n{contents}");
    stream.write_all(response.as_bytes()).unwrap();
}

fn main() {
    let listener = TcpListener::bind("localhost:7878").unwrap();

    println!("Server started");

    for stream in listener.incoming() {
        let stream = stream.unwrap();

        handle_connection(stream);
    }
}
