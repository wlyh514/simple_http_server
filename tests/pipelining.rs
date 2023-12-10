#[cfg(test)]
mod tests {
    use std::{
        io::{prelude::*, BufReader, Write},
        net::TcpStream,
        sync::mpsc::{self, Sender},
        thread,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    fn send_slow_request(mut stream: TcpStream) {
        stream
            .write_all(
                "GET /slow HTTP/1.1\r\nHost: localhost:7878\r\nConnection: keep-alive\r\n\r\n"
                    .as_bytes(),
            )
            .unwrap();
        println!("GET /slow request sent.");
    }

    enum RespListenerSignal {
        NewResp,
        Stop,
    }
    fn listen_to_response(stream: TcpStream, signal_tx: Sender<RespListenerSignal>) {
        let buf_reader = BufReader::new(stream);
        for line in buf_reader.lines().map(|result| result.unwrap()) {
            if line.starts_with("HTTP/1.1") {
                println!("Response Received");
                signal_tx.send(RespListenerSignal::NewResp).unwrap();
            }
            println!("{line}");
        }
        println!("Connection closed");
        signal_tx.send(RespListenerSignal::Stop).unwrap();
    }

    #[test]
    fn test_pipelining() {
        let stream = TcpStream::connect("localhost:7878").unwrap();
        let (signal_tx, signal_rx) = mpsc::channel::<RespListenerSignal>();

        let stream_cp = stream.try_clone().unwrap();
        let _resp_listener_handler =
            thread::spawn(move || listen_to_response(stream_cp, signal_tx));

        const NUM_REQUESTS: u32 = 3;

        for _ in 0..NUM_REQUESTS {
            send_slow_request(stream.try_clone().unwrap());
        }

        let mut prev_response_at = SystemTime::from(UNIX_EPOCH);
        for i in 0..NUM_REQUESTS {
            match signal_rx.recv().unwrap() {
                RespListenerSignal::NewResp => {
                    let now = SystemTime::now();
                    if i > 0 {
                        assert!(
                            now.duration_since(prev_response_at).unwrap() < Duration::from_secs(10)
                        );
                    }
                    prev_response_at = SystemTime::now();
                }
                RespListenerSignal::Stop => assert!(false),
            }
        }
        drop(stream);
        // TODO: Test whether the responses are returned in the sending order of requests.
    }
}
