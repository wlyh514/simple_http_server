# Project Report

## Description of Project

## Team Member Contributions

### Glenn Ye

I worked on creating helper functions for compressing and decompressing headers using the HPACK algorithm, along with the relevant unit tests. I also contributed in our attemp to integrate TLS by creating a server certificate configuration and allowing the server to detect and respond to a client TLS handshake.

### Ali Abdoli
I worked on creating a simpler API for interacting with the response object. Allowing reponse.set similar to the node express implementation.

## How to Run and Test the Project

## Implementation Details

### src/main.rs

#### `type ResponseQueue`

Part of the unused HTTP/1.1 implementation.

This is a queue of threads that the server generates to handle each individual request (i.e. process a response).

#### `enum RespHandlerSignal`

Part of the unused HTTP/1.1 implementation.

This acts as a signal for the spawned request handling threads. `NewResp` means that the thread should generate a response and `Finished` means that the thread is finished processing the request.

#### `fn handle_connection(stream: TcpStream)`

Part of the unused HTTP/1.1 implementation.

This function handles an incoming connection by parsing the TCP stream to get the HTTP request. Then it spawns two new threads: one to handle the request by generating a response and one to send the response. Once this is done, the spawned thread signals that it has finished processing the request.

#### `fn handle_response(response_queue: Arc<Mutex<ResponseQueue>>,mut stream:TcpStream, resp_signal_rx: Receiver<RespHandlerSignal>,)`

Part of the unused HTTP/1.1 implementation.

If the thread receives a `NewResp` signal, it sends the generated response to the TCP stream for the client to receive. If the thread receives a `Finished` signal, it exits.

#### `fn handle_request(http_request: Vec<String>, resp_signal_tx: Sender<RespHandlerSignal>) -> String`

Part of the unused HTTP/1.1 implementation.

Processes the HTTP request and generates a response. Then it signals the corresponding response thread to send the response.

#### `fn request_handler(req: http::HTTPRequest) -> http::HTTPResponse`

Handles a request by generating a HTTP response, consisting of a status code,
headers, and a body. The response is then returned to the caller.

#### `fn main()`

Following a common programming convention, this function is the entry point of the program. It initializes an address for the HTTP server to listen on (`localhost:7878`) and a TCP listener to listen for incoming TCP packets that
represent clients wanting to connect to the server. Then for each incoming connection, we use an instance of the HTTP server we implemented to handle the connection.

### src/http.rs

### src/http2.rs

### src/tls.rs

### src/http2/connection.rs

### src/http2/frames.rs

### src/http2/server.rs

### src/http2/stream.rs

### src/http2/window.rs


## Concluding Remarks
