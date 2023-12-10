use std::{io::{BufReader, Read, Write}, sync::{atomic::{AtomicUsize, Ordering}, Arc, Mutex, mpsc}, net::TcpStream};
use ::std::{thread, time};

use bytes::{Bytes, BytesMut};
use rustls::ServerConnection;

use crate::{http::ReqHandlerFn, http2::{frames::SettingsFlags, connection::{SettingsIdentifier, self}}};

use super::{connection::{Connection, SettingsMap}, frames::{Frame, FrameBody}};

pub type TlsStream<'a> = rustls::Stream<'a, ServerConnection, TcpStream>;
pub fn tls_read_exact_timeout(tls: &mut TlsStream, mut buf: &mut [u8], size: usize, timeout: time::Duration) -> Option<Result<usize, ()>> {
    let (timer_tx, timer_rx) = mpsc::channel();
    thread::spawn(move || {
        thread::sleep(timeout);
        let _ = timer_tx.send(());
    });

    let mut bytes_read = 0;
    let mut byte: [u8; 1] = [0];
    while bytes_read < size {
        match timer_rx.try_recv() {
            Ok(_) => {
                if bytes_read == 0 {
                    return None
                }
                if bytes_read != size {
                    return Some(Err(()))
                }
            }
            Err(_) => {}
        };
        let result = tls.read(&mut byte);
        match result {
            Ok(size) => {
                if size == 1 {
                    if let Err(_) = buf.write(&byte) {
                        return Some(Err(()));
                    }
                    bytes_read += 1;
                }
            }, 
            Err(_) => return Some(Err(())),
        }
    };
    Some(Ok(bytes_read))
}
// pub struct TlsStream {
//     connection: ServerConnection, 
//     tcp_stream: TcpStream, 
//     tls_stream: rustls::Stream<ServerConnection, TcpStream>,
// }
// impl TlsStream {
//     pub fn new(mut connection: ServerConnection, mut tcp_stream: TcpStream) -> Self {
//         let tls_stream = rustls::Stream::new(&mut connection, &mut tcp_stream);
//         Self {connection, tcp_stream, tls_stream}
//     }
// }
pub struct Server<T: ReqHandlerFn + Copy + 'static> {
    handler: T,
    connection_count: AtomicUsize,
}

impl<T: ReqHandlerFn + Copy + 'static> Server<T> {
    pub fn new(handler: T) -> Self {
        Self { handler, connection_count: AtomicUsize::new(0) }
    }

    pub fn handle_connection(&self, tls_connection: ServerConnection, tcp_stream: TcpStream) {
        // section 3.4: Starting HTTP/2 with prior knowledge
        // TRY TODO: Support other starting methods
        let connection_count = self.connection_count.fetch_add(1, Ordering::SeqCst);
        let handler_cp = self.handler;

        let handle = thread::spawn(move || {
            Server::<T>::_handle_connection(connection_count, tls_connection, tcp_stream, handler_cp)
        });
    }

    fn _handle_connection(connection_id: usize, mut tls_connection: ServerConnection, mut tcp_stream: TcpStream, handler: T) -> Option<()> {
        println!("Connection {connection_id} received");
        let mut tls_stream = rustls::Stream::new(&mut tls_connection, &mut tcp_stream);
        // let tls_stream = Arc::new(Mutex::new(tls_stream));

        // Receive client preface
        let mut preface_starter = [0; 24];
        tls_stream.read_exact(&mut preface_starter).ok()?;
        // println!("Preface read {:#?}", preface_starter);
        let preface_starter = String::from_utf8(preface_starter.into()).ok()?;
        if preface_starter != "PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n" {
            return None
        }
        println!("Client preface OK");

        let settings = match Frame::try_read_from_buf(&mut tls_stream) {
            Ok(frame) => {
                match frame.is_valid() {
                    Err(err) => {
                        println!("{err}");
                        return None
                    },
                    _ => {}
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
        let mut server_settings = SettingsMap::default();
        server_settings.set(SettingsIdentifier::EnablePush, 0).ok()?;   // Since curl does not support server pushing

        {
            // let mut tls_stream = tls_stream.lock().unwrap();
            // Send server preface
            let server_preface_frame = Frame::new(0, 0, FrameBody::Settings(server_settings.clone().into()));
            let server_preface_bytes: Bytes = server_preface_frame.try_into().ok()?;
            tls_stream.write_all(server_preface_bytes.as_ref()).ok()?;
            tls_stream.flush().unwrap();
            println!("Server preface sent");

            // Send ack
            let ack_frame = Frame::new(0, SettingsFlags::ACK.bits(), FrameBody::Settings(vec![]));
            let ack_frame_bytes: Bytes = ack_frame.try_into().ok()?;
            tls_stream.write_all(ack_frame_bytes.as_ref()).ok()?;

            tls_stream.flush().unwrap();
        }
        
        // Create connection struct
        let connection: Connection<T> = Connection::new(handler, server_settings, settings, connection_id);
        println!("Connection {connection_id} Established");
        connection.run(tls_stream);
        None
    }
}

mod test {
    use crate::http::{HTTPRequest, HTTPResponse};
    use super::Server;

    fn _sample_handler(_: HTTPRequest) -> HTTPResponse {
        HTTPResponse::default()
    }

    fn _test_traits() {
        let _server = Server::new(_sample_handler);
    }
}