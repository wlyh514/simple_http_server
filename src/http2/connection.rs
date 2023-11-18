use super::{frames::Frame, stream::Stream};
use std::{collections::HashMap, net::TcpStream};

pub struct Connection {
    tcp_stream: TcpStream,
    settings: SettingsMap,
    stream_counter: u32,
}
impl Connection {
    pub fn new(tcp_stream: TcpStream) -> Connection {
        Connection {
            tcp_stream,
            settings: SettingsMap::new(),
            stream_counter: 1,
        }
    }

    pub fn new_stream() -> Stream {
        // TODO: Implement this
    }

    pub fn read_frame() -> Frame {
        // TODO: Implement this
    }
}

/// See RFC7540 section 6.5
#[repr(u16)]
pub enum Settings {
    HeaderTableSize = 0x1,
    EnablePush = 0x2,
    MaxCurrentStream = 0x3,
    InitialWindowSize = 0x4,
    MaxFrameSize = 0x5,
    MaxHeaderListSize = 0x6,
}
type SettingsFrame = [u8; 6];
pub struct SettingsMap(HashMap<u16, u32>);
impl SettingsMap {
    pub fn new() -> SettingsMap {
        let map: HashMap<u16, u32> = HashMap::new();
        SettingsMap(map)
    }

    pub fn set_from_frame(&mut self, frame: SettingsFrame) {
        let identifier = u16::from_be_bytes([frame[0], frame[1]]);
        let value = u32::from_be_bytes([frame[2], frame[3], frame[4], frame[5]]);
        self.0.insert(identifier, value);
    }

    pub fn get(&self, field: Settings) -> Option<&u32> {
        self.0.get(&(field as u16))
    }
}
