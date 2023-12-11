# Project Report

This is an implementation of HTTP/1.1 and HTTP/2 server in Rust. Emphasizing on HTTP/2 and adhering to the [RFC7540](https://httpwg.org/specs/rfc7540.html) standard. Implemented feaures includes:

- HTTP/1.1:
  - Barebone HTTP message serialization and deserialization
  - Pipelining
- HTTP/2:
  - Multiplexing
  - Maintaining Connections, Streams and Frames
  - Flow-control
  - Error handling
  - Converting HTTP messages into/from HTTP/2 frames
  - ...any other mandatory feature for the server as described in [RFC7540](https://httpwg.org/specs/rfc7540.html), other than server pushing and prioritization.
- HTTP:
  - A simplistic API similar to [express.js](https://expressjs.com/) for describing endpoints.

External libraries are used for managing transport-layer connection and hpack ([RFC7541](https://httpwg.org/specs/rfc7541.html)).

[Video demonstration](https://www.youtube.com/watch?v=RpVLl11ABeE)

## Team Member Contributions

### Siyang Chen

I designed the overall architecture and implemented most of the HTTP/1.1 and HTTP/2 features.

### Glenn Ye

I worked on integrating HPACK for header compression. I also attempted TLS integration by creating a certificate configuration and responding to a client TLS handshake.

### Ali Abdoli

I worked on creating a simpler API for interacting with the response object. Allowing `response.set` similar to the node express implementation.

## Running and Testing

### Prerequisite

Rust is required to compile and run this project. Installation guide: [https://www.rust-lang.org/tools/install](https://www.rust-lang.org/tools/install).

### HTTP/1.1

Start the HTTP/1.1 server using `cargo run -- --http1`. Make sure no other processes are occupying TCP port 7878. This should yield no errors or warnings.

#### Basic Functionalities

Open `http://localhost:7878` in a browser. A welcoming page with blue background should be rendered. In the source tab of DevTools `css/index.css` and `js/index.js` should be seen loaded under `localhost:7878`.

Run `curl http://localhost:7878/js/index.js`, a piece of javascript code should be returned in the response.

Run `curl http://localhost:7878/ping`, the list of request headers should be returned in the response.

Run `curl http://localhost:7878/slow`, this should return `"Resource loaded after a while."` after 10 seconds.

#### Pipelining

With the HTTP/1.1 server running. In another terminal, run `cargo test pipelining`. This runs the pipelining test in the file `test/pipelining.rs`. This test should pass without error or panicking.

### HTTP/2

Start the HTTP/2 server using `cargo run -- --http2`. Make sure no other processes are occupying TCP port 7878.

#### h2spec

[h2spec](https://github.com/summerwind/h2spec) is a testing tool for HTTP/2 implementations. It compares the implementation's behaviours with expectations from [RFC7540](https://httpwg.org/specs/rfc7540.html). [Download](https://github.com/summerwind/h2spec/releases) the binary for your testing platform, or build from source.

When the HTTP/2 server is running, use `h2spec http2 -p 7878 -h localhost --strict` to run HTTP/2 tests on localhost port 7878 with strict mode. The following should be present at the end of the program output:

```
Finished in ____ seconds
95 tests, 95 passed, 0 skipped, 0 failed
```

A sample testing output can be found [here](./test_results.txt).

#### curl

We do not have TLS integrated into this implementation, therefore the `--http2-prior-knowledge` flag is required for testing. We noticed that this flag is not supported by some Windows `curl` applications, therefore it is recommended to curl the HTTP/2 server on a Linux or MacOS machine. All the curl tests in the HTTP/1.1 section (with `--http2-prior-knowledge` flag added) should yield the same result as in HTTP/1.1 tests. Additionally, `curl http://localhost:7878/ping` should include pseudo-headers `:path`, `:method`, `:authority` and `:scheme`.

## Project Overview

`main.rs`: Entry point of the server. Defines endpoints and listens to TCP connections.

`http.rs`: Definition of HTTP request, response, headers, handler functions etc. Both HTTP/1.1 and HTTP/2 servers depends on this module. This module is not responsible for any server-specific implementations.

`http1.rs`: HTTP/1.1 implementation. Responsible for serialization and deserialization of HTTP/1.1 messages, as well as maintaining the multithreaded execution of request handlers.

`http2.rs`: Module file for the http2 module, exposes only the `Server` struct to the user.

- `http2/server.rs`: Definition of HTTP/2 servers, owns a handler function and a connection counter, create a new thread for each incoming connection.
- `http2/connection.rs`: Implementation of HTTP/2 connections. Responsible for all network-level operations.
- `http2/stream.rs`: Implementation of HTTP/2 streams ([RFC7540 Section 5](https://httpwg.org/specs/rfc7540.html#StreamsLayer)). Each stream maintains its stream state machine (Section 5.1), the stream-level client flow-control window (5.2) and its process of assembling a request from frames (Section 8). This module does not directly interact with transport layer streams.
- `http2/frames.rs`: Definition of HTTP/2 frames ([RFC7540 Section 4, 6](https://httpwg.org/specs/rfc7540.html#FramingLayer)). Implements serialization and deserialization of frames.
- `http2/window.rs`: Implementation of HTTP/2 stream level flow-control windows ([RFC7540 Section 5.2](https://httpwg.org/specs/rfc7540.html#FlowControl)). A window struct keeps track of how much peer window size is left, and generates flow-controlled frames if the peer is capable of receiving it.
- `http2/error.rs`: Definition of HTTP/2 error codes ([RFC7540 Section 7](https://httpwg.org/specs/rfc7540.html#ErrorCodes)) and relevant utility data structures.
- `http2/settings.rs`: Definition of HTTP/2 stream/connection settings ([RFC7540 Section 6.5](https://httpwg.org/specs/rfc7540.html#ErrorCodes)) and relevant utility data structures.

`tls.rs`: TLS utilities, handles server keys and configurations. This is not used in the final version since we did not have TLS integrated properly with HTTP/2. See the Remarks section.

## Detailed Documentation

Detailed documentation of the code can be found [here](./DOC.md).

## Remarks

### Limitations

The following features are missing

- TLS: We tried to integrate TLS into our project, however sharing the TLS stream over two synchronous threads was way more complicated than we original anticipated. We had to compromise a large portion of our codebase and were constantly bugged by deadlocks and other synchronization issues, even with the help of Rust's concurrrency constructs. This can be found on the tls branch. Therefore we decided to not support TLS.
- Server Push: We decided not to use server push because it is not commonly supported, read more in [this blog post](https://developer.chrome.com/blog/removing-push).
- Prioritization: We decided this is an optional feature, and is only relavent when resources are scarce. This feature is not tested by h2spec.
- Complexed HTTP header semantics ([RFC9110](https://httpwg.org/specs/rfc9110.html)): There are a lot of different header semantics defined in RFC9110, supporting all of them will be too much work and is irrelevant to computer networking. We ended up treating header fields as simple string-to-string mappings.
- Implmentation of HPACK: We used a library to handle HTTP/2 header compression/decompression. Originally we intended to implement it from scratch, but then found it to be too complicated and not really relevant to the course materials. We wasted several hours on this.

### Lessons Learned

#### Use async programming in future network projects

Async frameworks reduces the overhead of spawning threads, and allows us to split a stream into a read-half and a write-half, and enables completing I/O tasks in a non-blocking manner ergonomically. This could drastically reduce the difficulty of integrating TLS.

#### Decouple network handling and protocol logic

We ended up having a bloated `http2/connection.rs` partially because it is both responsible for communicating with the underlying TCP stream, and maintaining the HTTP/2 connection states. The code can be cleaner if we decoupled them beforehand.

#### Make sure to understand all requirements before designing the architecture

We underestimated the complexity of maintaining flow-control windows, and spent quite a while to integrate it into our project. This also contributes to a bloated `connection.rs`.
