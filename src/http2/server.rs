use ::std::{net::TcpStream, thread};

use crate::http::{ReqHandlerFn};

use super::connection::Connection;


pub struct Server <T: ReqHandlerFn> {
    handler: T,
}

impl<T: ReqHandlerFn> Server<T> {

    pub fn new(handler: T) -> Self {
        Self { handler }
    }

    pub fn handle_connection(self, stream: TcpStream) {
        // TODO

        // Create connection struct
        let connection = Connection::new(stream);
        thread::spawn(move || {
            connection.run();
            // Kill connection
        });
    }


}