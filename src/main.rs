use clap::Parser;
use std::{
    net::{TcpListener, TcpStream},
    thread::{self},
    time::Duration,
};

pub mod http;
pub mod http1_1;
pub mod http2;

use http::ResponseStatus;

fn request_handler(req: http::HTTPRequest, res: &mut http::HTTPResponse) {
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/") => {
            res.file(&format!("static/index.html"));
        }
        (_, "/slow") => {
            // Simulate long processing time
            thread::sleep(Duration::from_secs(10));
            res.text(String::from("Resouce loaded after a while. "));
        }
        ("GET", "/ping") => {
            res.text(format!("{:#?}", req.headers));
        }
        ("GET", path) => {
            res.file(&format!("static/{path}"));
        }
        _ => {
            res.status(ResponseStatus::NotFound);
        }
    };
}

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Run HTTP/1.1 pipelining server.
    #[arg(long, default_value_t = false)]
    http1: bool,

    /// Run HTTP/2 multiplexing server. This is the default behaviour.
    #[arg(long, default_value_t = true)]
    http2: bool,
}

fn main() {
    let host: &str = "localhost:7878";
    let listener: TcpListener = TcpListener::bind(host).unwrap();
    let h2_server = http2::Server::new(request_handler);
    let h1_server = http1_1::Server::new(request_handler);

    let args = Args::parse();
    let protocol = if args.http1 { "HTTP/1.1" } else { "HTTP/2" };
    println!("{protocol} Server started on {host}");

    // Start a TLS server that waits for incoming connections.
    for stream in listener.incoming() {
        let stream: TcpStream = stream.unwrap();
        println!("new connection in main");
        if args.http1 {
            h1_server.handle_connection(stream);
        } else {
            h2_server.handle_connection(stream);
        }
    }
}
