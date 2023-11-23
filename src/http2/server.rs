use ::std::{net::TcpStream, thread};

use crate::http::ReqHandlerFn;

use super::connection::Connection;


pub struct Server<T: ReqHandlerFn + Sync + Send> {
    handler: T,
}

impl<T: ReqHandlerFn + Sync + Send> Server<T> {
    pub fn new(handler: T) -> Self {
        Self { handler }
    }

    pub fn handle_connection(&self, stream: TcpStream) {
        // TODO

        // Create connection struct
        let connection = Connection::new(stream, self.handler);
        thread::spawn(move || connection.run() /* Maybe kill connection afterwards? */);
    }


}

mod test {
    use crate::http::{HTTPRequest, HTTPResponse};
    use super::Server;

    fn sample_handler(request: HTTPRequest) -> HTTPResponse {
        HTTPResponse::default()
    }

    fn test_traits() {
        let server = Server::new(sample_handler);
    }
}