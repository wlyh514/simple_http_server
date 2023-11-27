use std::io::{BufReader, Read, Write};
use ::std::{net::TcpStream, thread};

use bytes::Bytes;

use crate::http::ReqHandlerFn;

use super::{connection::{Connection, SettingsMap}, frames::{Frame, FrameBody}};


pub struct Server<T: ReqHandlerFn + Sync + Send + Copy + 'static> {
    handler: T,
}

impl<T: ReqHandlerFn + Sync + Send + Copy> Server<T> {
    pub fn new(handler: T) -> Self {
        Self { handler }
    }

    pub fn handle_connection(&self, mut stream: TcpStream) -> Option<()> {
        // section 3.4: Starting HTTP/2 with prior knowledge
        // TRY TODO: Support other starting methods

        // Receive a preface
        let read_stream = stream.try_clone().ok()?;
        let mut reader = BufReader::new(read_stream);
        let mut preface_starter = [0; 24];
        reader.read_exact(&mut preface_starter);
        let preface_starter = String::from_utf8(preface_starter.into()).ok()?;
        if preface_starter != "PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n" {
            return None
        }

        let settings = match Frame::try_read_from_buf(reader) {
            Ok(frame) => {
                if frame.validate().is_err() {
                    return None
                }
                match frame.payload {
                    FrameBody::Settings(settings) => settings,
                    _ => { return None }
                }
            },
            Err(_) => {
                return None
            }
        };

        // Send a preface
        let server_preface_frame = Frame::new(0, 0, FrameBody::Settings(SettingsMap::default().into()));
        let server_preface_bytes: Bytes = server_preface_frame.try_into().ok()?;
        stream.write_all(&server_preface_bytes);

        // Create connection struct
        let connection = Connection::new(self.handler.clone(), settings);
        thread::spawn(move || connection.run(stream) /* Maybe kill connection afterwards? */);
        None
    }
}

mod test {
    use crate::http::{HTTPRequest, HTTPResponse};
    use super::Server;

    fn sample_handler(_: HTTPRequest) -> HTTPResponse {
        HTTPResponse::default()
    }

    fn test_traits() {
        let _server = Server::new(sample_handler);
    }
}