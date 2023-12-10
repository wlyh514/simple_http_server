mod connection;
mod error;
mod frames;
mod server;
mod settings;
mod stream;
mod window;

pub type Server<T> = server::Server<T>;
