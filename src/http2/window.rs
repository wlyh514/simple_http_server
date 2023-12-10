use super::frames::{DataFlags, Frame, FrameBody};
use bytes::Bytes;

pub struct Window {
    data_buffer: Option<Bytes>,
    available_size: i64,
}

impl Window {
    pub fn new(size: i64) -> Window {
        Window {
            data_buffer: None,
            available_size: size,
        }
    }

    pub fn window_update(&mut self, increment: i64) -> Result<bool, ()> {
        let new_size = self.available_size + increment;
        if new_size > 2147483647 {
            return Err(());
        }
        self.available_size = new_size;
        Ok(!self.data_buffer.is_some() && self.available_size > 0)
    }

    pub fn push_data(&mut self, data: Bytes) -> Result<(), ()> {
        if self.data_buffer.is_some() {
            Err(())
        } else {
            self.data_buffer = Some(data);
            Ok(())
        }
    }

    pub fn make_data_frames(
        &mut self,
        max_frame_size: usize,
        stream_id: u32,
    ) -> Option<Vec<Frame>> {
        if self.available_size <= 0 {
            return None;
        }
        match &mut self.data_buffer {
            Some(data) => {
                let mut frames = Vec::new();

                let mut remaining_bytes = usize::min(data.len(), self.available_size as usize);
                while remaining_bytes > 0 {
                    let bytes_taken = usize::min(remaining_bytes, max_frame_size);

                    let mut flags = DataFlags::from_bits_retain(0);
                    if remaining_bytes < max_frame_size && remaining_bytes == data.len() {
                        flags |= DataFlags::END_STREAM;
                    }

                    let data_frag = data.split_to(bytes_taken);
                    frames.push(Frame::new(
                        stream_id,
                        flags.bits(),
                        FrameBody::Data {
                            pad_length: None,
                            data: data_frag,
                        },
                    ));

                    remaining_bytes -= bytes_taken;
                    self.available_size -= bytes_taken as i64;
                }
                if data.is_empty() {
                    self.data_buffer = None;
                }

                Some(frames)
            }
            None => None,
        }
    }
}
