use ::std::net::TcpStream;

use super::frames::Frame;

pub struct Stream {
    id: i32,
    state: StreamState,
    tcp_stream: TcpStream,
}
enum StreamState {
    Idle,
    Open,
    ReservedLocal,
    ReservedRemote,
    HalfClosedRemote,
    HalfClosedLocal,
    Closed,
}

impl Stream {
    fn new(id: i32, tcp_stream: TcpStream) -> Stream {
        Stream {
            id,
            state: StreamState::Idle,
            tcp_stream,
        }
    }

    fn recv(frame: Frame) {
        // TODO: Implement this
    }

    fn send(frame: Frame) {
        // TODO: Implement this
    }
}
