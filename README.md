# Simple HTTP Server

This is a final project for the course CSCD58 Fall 2023, a HTTP/1.1 and HTTP/2 server implementation.

## Report

[Report markdown file](./REPORT.md)

## Documentation

[Documentation markdown file](./DOC.md)

## Video Demo

[YouTube: CSCD58 Final Demo](https://www.youtube.com/watch?v=RpVLl11ABeE)

```rust
...
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
        if args.http1 {
            h1_server.handle_connection(stream);
        } else {
            h2_server.handle_connection(stream);
        }
    }
}

```
