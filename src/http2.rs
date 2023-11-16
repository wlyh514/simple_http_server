use std::{collections::HashMap, str::Bytes, borrow::BorrowMut};
use crate::http::{ Headers };

fn compress_headers(headers: Headers) -> Bytes<'static> {

}

fn decompress_headers(header_block: Bytes<'static>) -> Headers {

}

/// See RFC7540 section 6.5
#[repr(u16)]
enum Settings {
    HeaderTableSize    = 0x1,
    EnablePush         = 0x2, 
    MaxCurrentStream   = 0x3, 
    InitialWindowSize  = 0x4, 
    MaxFrameSize       = 0x5, 
    MaxHeaderListSize  = 0x6, 
}
type SettingsFrame = [u8; 6];
struct SettingsMap(HashMap<u16, u32>);
impl SettingsMap {
    pub fn set_from_frame(&mut self, frame: SettingsFrame) {
        let identifier = u16::from_be_bytes([frame[0], frame[1]]);
        let value = u32::from_be_bytes([frame[2], frame[3], frame[4], frame[5]]); 
        self.0.insert(identifier, value);
    }

    pub fn get(&self, field: Settings) -> Option<&u32> {
        self.0.get(&(field as u16))
    }
}
