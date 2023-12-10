use ::std::{net::TcpStream, thread};
use std::{
    io::{BufReader, BufWriter, Read, Write},
    sync::atomic::{AtomicUsize, Ordering},
};

use bytes::Bytes;

use crate::http::ReqHandlerFn;

use super::{
    connection::Connection,
    frames::{Frame, FrameBody, SettingsFlags},
    settings::{SettingsIdentifier, SettingsMap},
};

pub struct Server<T: ReqHandlerFn + Copy + 'static> {
    handler: T,
    connection_count: AtomicUsize,
}

impl<T: ReqHandlerFn + Copy + 'static> Server<T> {
    pub fn new(handler: T) -> Self {
        Self {
            handler,
            connection_count: AtomicUsize::new(0),
        }
    }

    pub fn handle_connection(&self, stream: TcpStream) {
        // section 3.4: Starting HTTP/2 with prior knowledge
        // TRY TODO: Support other starting methods
        let connection_count = self.connection_count.fetch_add(1, Ordering::SeqCst);
        let handler_cp = self.handler;

        thread::spawn(move || {
            Server::<T>::_handle_connection(connection_count, stream, handler_cp)
        });
    }

    fn _handle_connection(connection_id: usize, stream: TcpStream, handler: T) -> Option<()> {
        println!("Connection {connection_id} received");

        // Receive client preface
        let read_stream = stream.try_clone().ok()?;
        let mut tcp_reader = BufReader::new(read_stream);
        let mut tcp_writer = BufWriter::new(stream);

        let mut preface_starter = [0; 24];
        tcp_reader.read_exact(&mut preface_starter).ok()?;
        let preface_starter = String::from_utf8(preface_starter.into()).ok()?;
        if preface_starter != "PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n" {
            return None;
        }

        let settings = match Frame::try_read_from_buf(&mut tcp_reader) {
            Ok(frame) => {
                match frame.is_valid() {
                    Err(err) => {
                        println!("{err}");
                        return None;
                    }
                    _ => {}
                }
                match frame.payload {
                    FrameBody::Settings(settings) => settings,
                    _ => return None,
                }
            }
            Err(_) => return None,
        };
        let mut server_settings = SettingsMap::default();
        server_settings
            .set(SettingsIdentifier::EnablePush, 0)
            .ok()?; // Since curl does not support server pushing

        // Send server preface
        let server_preface_frame =
            Frame::new(0, 0, FrameBody::Settings(server_settings.clone().into()));
        let server_preface_bytes: Bytes = server_preface_frame.try_into().ok()?;
        tcp_writer.write_all(server_preface_bytes.as_ref()).ok()?;
        tcp_writer.flush().unwrap();

        // Send ack
        let ack_frame = Frame::new(0, SettingsFlags::ACK.bits(), FrameBody::Settings(vec![]));
        let ack_frame_bytes: Bytes = ack_frame.try_into().ok()?;
        tcp_writer.write_all(ack_frame_bytes.as_ref()).ok()?;

        tcp_writer.flush().unwrap();

        // Create connection struct
        let connection: Connection<T> =
            Connection::new(handler, server_settings, settings, connection_id);
        println!("Connection {connection_id} Established");
        connection.run(tcp_reader, tcp_writer);
        None
    }
}
